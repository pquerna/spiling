// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounds on the stored homogeneous coefficients, without rounded knot insertion.
//! Closed queries retain both incident one-sided jets at every original knot.
//! Axis-constant homogeneous components retain exact zero derivatives on each
//! piece; recentering still carries weight derivatives through rational division.

use super::interval::Interval;
use crate::{GeometryError, GeometryWorkBudget};
use solidcore::nurbs::{b_spline_surface::BSplineSurface, srf_basic::Surface};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

pub const MAX_INTERVAL_DEGREE: usize = 32;
pub const BASIS_SCRATCH_BYTES: usize =
    std::mem::size_of::<[[Interval; 4]; MAX_INTERVAL_DEGREE + 2]>();
pub const SURFACE_EVALUATION_SCRATCH_BYTES: usize = 3 * BASIS_SCRATCH_BYTES
    + std::mem::size_of::<[[[Interval; 4]; 4]; 4]>()
    + 3 * std::mem::size_of::<SurfaceTensor>()
    + std::mem::size_of::<([f64; 3], [f64; 4], Option<Interval>, [usize; 8])>()
    + std::mem::size_of::<([[f64; 4]; 3], [u8; 2])>()
    + std::mem::size_of::<(
        DerivativeControls,
        Option<[Interval; 3]>,
        [Interval; 3],
        [Interval; 2],
        [usize; 8],
    )>();
const ZERO: Interval = Interval::point(0.0);
type DerivativeControls = [[[Interval; 3]; 4]; 4];

/// Convex enclosure of one polynomial derivative on one original knot cell.
/// Differentiate literal coefficients before evaluating basis intervals: a
/// constant derivative must not inherit an independent partition-of-unity sum.
/// The reduced-degree basis is nonnegative and sums to one on this cell.
fn polynomial_derivative_hull(
    surface: &Surface,
    span: [usize; 2],
    derivative: [usize; 2],
    controls: &mut DerivativeControls,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 3], GeometryError> {
    let (p, q) = surface.degrees();
    let [a, b] = derivative;
    if a > 3 || b > 3 {
        return Err(conditioning(
            "polynomial derivative stencil exceeds order three",
        ));
    }
    if a > p || b > q {
        return Ok([ZERO; 3]);
    }
    let (uk, vk) = (surface.u_knots(), surface.v_knots());
    let mut hull: Option<[Interval; 3]> = None;
    for i in span[0] - p..=span[0] - a {
        for j in span[1] - q..=span[1] - b {
            for (x, row) in controls.iter_mut().enumerate().take(a + 1) {
                for (y, point) in row.iter_mut().enumerate().take(b + 1) {
                    work.charge(1, cancel)?;
                    let c = surface
                        .cpt(i + x, j + y)
                        .map_err(|_| conditioning("missing polynomial derivative coefficient"))?;
                    *point = [c.x, c.y, c.z].map(Interval::point);
                }
            }
            // Left-to-right updates read the next, not-yet-updated coefficient
            // in each pass. Reduced knot vectors remove one slot at each end.
            for order in 1..=a {
                for x in 0..=a - order {
                    let denominator =
                        Interval::point(uk[i + x + p + 1]).sub(Interval::point(uk[i + x + order]));
                    if denominator.lo <= 0.0 {
                        return Err(conditioning(
                            "polynomial u derivative has no positive knot support",
                        ));
                    }
                    let factor = Interval::point((p - order + 1) as f64).div(denominator)?;
                    let (left, right) = controls.split_at_mut(x + 1);
                    for (current, next) in left[x].iter_mut().zip(&right[0]).take(b + 1) {
                        work.charge(1, cancel)?;
                        *current = std::array::from_fn(|k| next[k].sub(current[k]).mul(factor));
                    }
                }
            }
            for order in 1..=b {
                for y in 0..=b - order {
                    work.charge(1, cancel)?;
                    let denominator =
                        Interval::point(vk[j + y + q + 1]).sub(Interval::point(vk[j + y + order]));
                    if denominator.lo <= 0.0 {
                        return Err(conditioning(
                            "polynomial v derivative has no positive knot support",
                        ));
                    }
                    let factor = Interval::point((q - order + 1) as f64).div(denominator)?;
                    controls[0][y] = std::array::from_fn(|k| {
                        controls[0][y + 1][k].sub(controls[0][y][k]).mul(factor)
                    });
                }
            }
            let value = controls[0][0];
            for coordinate in value {
                Interval::new(coordinate.lo, coordinate.hi)?;
            }
            hull = Some(hull.map_or(value, |old| std::array::from_fn(|k| old[k].hull(value[k]))));
        }
    }
    hull.ok_or_else(|| conditioning("polynomial derivative has no active control stencil"))
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceEnclosure {
    pub position: [Interval; 3],
    pub du: [Interval; 3],
    pub dv: [Interval; 3],
    pub duu: [Interval; 3],
    pub duv: [Interval; 3],
    pub dvv: [Interval; 3],
}
impl SurfaceEnclosure {
    pub fn hull(self, other: Self) -> Self {
        Self {
            position: std::array::from_fn(|i| self.position[i].hull(other.position[i])),
            du: std::array::from_fn(|i| self.du[i].hull(other.du[i])),
            dv: std::array::from_fn(|i| self.dv[i].hull(other.dv[i])),
            duu: std::array::from_fn(|i| self.duu[i].hull(other.duu[i])),
            duv: std::array::from_fn(|i| self.duv[i].hull(other.duv[i])),
            dvv: std::array::from_fn(|i| self.dvv[i].hull(other.dvv[i])),
        }
    }
}
pub(super) fn conditioning(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ConditioningFailure, message)
}

pub(crate) fn validate_knots(
    knots: &[f64],
    degree: usize,
    count: usize,
    range: [f64; 2],
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(1, cancel)?;
    Interval::new(range[0], range[1])?;
    if degree > MAX_INTERVAL_DEGREE {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "interval spline degree limit exceeded",
        ));
    }
    if count <= degree || count.checked_add(degree + 1) != Some(knots.len()) {
        return Err(conditioning("invalid original spline knot/control counts"));
    }
    let mut multiplicity = 1;
    for pair in knots.windows(2) {
        work.charge(1, cancel)?;
        if !pair[0].is_finite() || !pair[1].is_finite() || pair[0] > pair[1] {
            return Err(conditioning("nonfinite or unordered original knots"));
        }
        multiplicity = if pair[0] == pair[1] {
            multiplicity + 1
        } else {
            1
        };
        if multiplicity > degree + 1 {
            return Err(conditioning(
                "original knot multiplicity exceeds degree plus one",
            ));
        }
    }
    if knots[degree] >= knots[count] || range[0] < knots[degree] || range[1] > knots[count] {
        return Err(conditioning(&format!(
            "query extends outside complete active spline domain: query={range:?}, active=[{},{}], degree={degree}, controls={count}",
            knots[degree], knots[count]
        )));
    }
    Ok(())
}

/// Cox-de Boor on one literal nonzero knot span. Recurrences and knot differences
/// are interval operations; no Bezier conversion or rounded control net is trusted.
pub(crate) fn basis_jet(
    knots: &[f64],
    degree: usize,
    span: usize,
    range: [f64; 2],
    order: usize,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[[Interval; 4]; MAX_INTERVAL_DEGREE + 2], GeometryError> {
    let t = Interval::new(range[0], range[1])?;
    let mut values = [[ZERO; 4]; MAX_INTERVAL_DEGREE + 2];
    values[degree][0] = Interval::point(1.0);
    let start = span - degree;
    for d in 1..=degree {
        for j in degree - d..=degree {
            work.charge(1, cancel)?;
            let i = start + j;
            let mut value = [ZERO; 4];
            for (next, a, b, slope) in [(j, i, i + d, 1.0), (j + 1, i + 1, i + d + 1, -1.0)] {
                if knots[a] == knots[b] {
                    continue;
                }
                let denominator = Interval::point(knots[b]).sub(Interval::point(knots[a]));
                let factor = if slope > 0.0 {
                    t.sub(Interval::point(knots[a]))
                } else {
                    Interval::point(knots[b]).sub(t)
                }
                .div(denominator)?;
                let derivative = Interval::point(slope).div(denominator)?;
                for n in 0..=order {
                    value[n] = value[n].add(factor.mul(values[next][n]));
                    if n > 0 {
                        value[n] = value[n].add(
                            Interval::point(n as f64)
                                .mul(derivative)
                                .mul(values[next][n - 1]),
                        );
                    }
                }
            }
            // Basis positivity and partition of unity are exact on this span.
            value[0] = value[0].intersect(Interval { lo: 0.0, hi: 1.0 })?;
            values[j] = value;
        }
    }
    Ok(values)
}

pub(crate) fn binomial(n: usize, k: usize) -> f64 {
    match (n, k) {
        (_, 0) => 1.0,
        (1, 1) => 1.0,
        (2, 1) => 2.0,
        (2, 2) => 1.0,
        (3, 1) | (3, 2) => 3.0,
        (3, 3) => 1.0,
        _ => 0.0,
    }
}

pub fn evaluate_nurbs_surface_jet(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    evaluate_nurbs_surface_order(surface, uv, 2, work, cancel)
}

pub(crate) type SurfaceTensor = [[[Interval; 3]; 4]; 4];

pub(super) fn evaluate_nurbs_surface_order(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    let j = evaluate_nurbs_surface_tensor(surface, uv, order, work, cancel)?;
    Ok(SurfaceEnclosure {
        position: j[0][0],
        du: j[1][0],
        dv: j[0][1],
        duu: j[2][0],
        duv: j[1][1],
        dvv: j[0][2],
    })
}

pub(super) fn evaluate_nurbs_surface_tensor(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    order: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceTensor, GeometryError> {
    if order > 3 {
        return Err(conditioning("surface derivative order exceeds three"));
    }
    let (p, q) = surface.degrees();
    let uk = surface.u_knots();
    let vk = surface.v_knots();
    let indices = surface.indices();
    let nu = indices.n + 1;
    let nv = indices.m + 1;
    let rational = surface.is_rational();
    validate_knots(uk, p, nu, uv[0], work, cancel)?;
    validate_knots(vk, q, nv, uv[1], work, cancel)?;
    let mut result: Option<SurfaceTensor> = None;
    let mut derivative_controls = [[[ZERO; 3]; 4]; 4];
    for us in p..nu {
        work.charge(1, cancel)?;
        if uk[us] == uk[us + 1] || uk[us] > uv[0][1] || uk[us + 1] < uv[0][0] {
            continue;
        }
        let ur = [uv[0][0].max(uk[us]), uv[0][1].min(uk[us + 1])];
        let ub = basis_jet(uk, p, us, ur, order, work, cancel)?;
        for vs in q..nv {
            work.charge(1, cancel)?;
            if vk[vs] == vk[vs + 1] || vk[vs] > uv[1][1] || vk[vs + 1] < uv[1][0] {
                continue;
            }
            let vr = [uv[1][0].max(vk[vs]), uv[1][1].min(vk[vs + 1])];
            let vb = basis_jet(vk, q, vs, vr, order, work, cancel)?;
            let mut h = [[[ZERO; 4]; 4]; 4];
            let anchor = surface
                .cpt(us - p, vs - q)
                .map_err(|_| conditioning("missing native anchor coefficient"))?;
            let anchor_w = if rational { anchor.w } else { 1.0 };
            // Any finite binary64 constant is a valid recentering origin.
            // A zero anchor weight does not imply a zero curve denominator.
            let origin = if anchor_w != 0.0 {
                [
                    anchor.x / anchor_w,
                    anchor.y / anchor_w,
                    anchor.z / anchor_w,
                ]
            } else {
                [0.0; 3]
            };
            for v in origin {
                Interval::new(v, v)?;
            }
            let mut weight_hull: Option<Interval> = None;
            // On this original knot rectangle, a homogeneous component that
            // repeats along an active axis has zero derivatives in that axis.
            // Compare RAW coefficients, before recentering widens them.
            let mut invariant = [if order == 0 { 0_u8 } else { 15_u8 }; 2];
            #[expect(
                clippy::needless_range_loop,
                reason = "Keep u/v coefficient, basis and invariant-row indices aligned throughout the homogeneous tensor accumulation"
            )]
            for i in 0..=p {
                let mut row_first = [0.0; 4];
                for j in 0..=q {
                    work.charge(1, cancel)?;
                    let c = if i == 0 && j == 0 {
                        anchor
                    } else {
                        surface
                            .cpt(us - p + i, vs - q + j)
                            .map_err(|_| conditioning("missing native control point"))?
                    };
                    if (c.w != solidcore::nurbs::math::NOW) != rational
                        || c.z == solidcore::nurbs::math::NOZ
                    {
                        return Err(conditioning(
                            "inconsistent native homogeneous surface coefficients",
                        ));
                    }
                    let w = if rational { c.w } else { 1.0 };
                    let coeff = [c.x, c.y, c.z, w];
                    for a in coeff {
                        Interval::new(a, a)?;
                    }
                    if i > 0 && invariant[0] != 0 {
                        work.charge(1, cancel)?;
                        let first = surface.cpt(us - p, vs - q + j).map_err(|_| {
                            conditioning("missing native invariant-axis coefficient")
                        })?;
                        let first = [
                            first.x,
                            first.y,
                            first.z,
                            if rational { first.w } else { 1.0 },
                        ];
                        for k in 0..4 {
                            if coeff[k] != first[k] {
                                invariant[0] &= !(1 << k);
                            }
                        }
                    }
                    if invariant[1] != 0 {
                        if j == 0 {
                            row_first = coeff;
                        } else {
                            for k in 0..4 {
                                if coeff[k] != row_first[k] {
                                    invariant[1] &= !(1 << k);
                                }
                            }
                        }
                    }
                    let wi = Interval::point(w);
                    weight_hull = Some(weight_hull.map_or(wi, |old| old.hull(wi)));
                    for a in 0..=order {
                        for b in 0..=order - a {
                            let basis = ub[i][a].mul(vb[j][b]);
                            for k in 0..4 {
                                let coefficient = if k < 3 {
                                    Interval::point(coeff[k])
                                        .sub(Interval::point(origin[k]).mul(wi))
                                } else {
                                    wi
                                };
                                h[a][b][k] = h[a][b][k].add(basis.mul(coefficient));
                            }
                        }
                    }
                }
            }
            if !rational {
                // Exact coefficient invariance also removes the zero-order
                // basis sum on that axis. Keeping a fictitious independent sum
                // would broaden every other derivative and even constant-height
                // positions on an unchanged extrusion.
                for k in 0..3 {
                    let fixed = [invariant[0] & (1 << k) != 0, invariant[1] & (1 << k) != 0];
                    match fixed {
                        [true, true] => h[0][0][k] = ZERO,
                        [false, true] => {
                            for a in 0..=order {
                                let mut sum = ZERO;
                                for (i, basis) in ub.iter().enumerate().take(p + 1) {
                                    work.charge(1, cancel)?;
                                    let c = surface.cpt(us - p + i, vs - q).map_err(|_| {
                                        conditioning(
                                            "missing axis-invariant polynomial coefficient",
                                        )
                                    })?;
                                    let value = [c.x, c.y, c.z][k];
                                    sum = sum.add(basis[a].mul(
                                        Interval::point(value).sub(Interval::point(origin[k])),
                                    ));
                                }
                                h[a][0][k] = sum;
                            }
                        }
                        [true, false] => {
                            for b in 0..=order {
                                let mut sum = ZERO;
                                for (j, basis) in vb.iter().enumerate().take(q + 1) {
                                    work.charge(1, cancel)?;
                                    let c = surface.cpt(us - p, vs - q + j).map_err(|_| {
                                        conditioning(
                                            "missing axis-invariant polynomial coefficient",
                                        )
                                    })?;
                                    let value = [c.x, c.y, c.z][k];
                                    sum = sum.add(basis[b].mul(
                                        Interval::point(value).sub(Interval::point(origin[k])),
                                    ));
                                }
                                h[0][b][k] = sum;
                            }
                        }
                        [false, false] => {}
                    }
                }
                #[expect(
                    clippy::needless_range_loop,
                    reason = "Only the triangular mixed-derivative catalog is initialized in the homogeneous weight component"
                )]
                for a in 0..=order {
                    for b in 0..=order - a {
                        h[a][b][3] = ZERO;
                    }
                }
                h[0][0][3] = Interval::point(1.0);
            }
            #[expect(
                clippy::needless_range_loop,
                reason = "The quotient recurrence reads lower-order tensor entries; preserve its total-order schedule and accumulation order"
            )]
            for total in 1..=order {
                for a in 0..=total {
                    let b = total - a;
                    let zero = (if a > 0 { invariant[0] } else { 0 })
                        | (if b > 0 { invariant[1] } else { 0 });
                    if zero & 8 != 0 {
                        h[a][b][3] = ZERO;
                    }
                    for k in 0..3 {
                        if zero & (1 << k) != 0 {
                            // D(H_k - origin_k W) = -origin_k D(W).
                            // A constant raw coordinate with varying weights
                            // is NOT a constant Cartesian coordinate.
                            h[a][b][k] = Interval::point(origin[k]).mul(h[a][b][3]).neg();
                        }
                    }
                }
            }
            h[0][0][3] = h[0][0][3]
                .intersect(weight_hull.ok_or_else(|| conditioning("empty control net"))?)?;
            if h[0][0][3].contains_zero() {
                return Err(conditioning(
                    "rational surface denominator not excluded from zero",
                ));
            }
            let mut jet = [[[ZERO; 3]; 4]; 4];
            for total in 0..=order {
                for a in 0..=total {
                    let b = total - a;
                    for k in 0..3 {
                        let mut value = h[a][b][k];
                        for i in 0..=a {
                            for j in 0..=b {
                                if i == 0 && j == 0 {
                                    continue;
                                }
                                value = value.sub(
                                    Interval::point(binomial(a, i) * binomial(b, j))
                                        .mul(h[i][j][3])
                                        .mul(jet[a - i][b - j][k]),
                                );
                            }
                        }
                        jet[a][b][k] = value.div(h[0][0][3])?;
                        Interval::new(jet[a][b][k].lo, jet[a][b][k].hi)?;
                    }
                }
            }
            for k in 0..3 {
                jet[0][0][k] = jet[0][0][k].add(Interval::point(origin[k]));
            }
            if !rational {
                for (a, row) in jet.iter_mut().enumerate().take(order + 1) {
                    for (b, derivative) in row.iter_mut().enumerate().take(order - a + 1) {
                        if a + b == 0 {
                            continue;
                        }
                        let hull = polynomial_derivative_hull(
                            surface,
                            [us, vs],
                            [a, b],
                            &mut derivative_controls,
                            work,
                            cancel,
                        )?;
                        for (value, bound) in derivative.iter_mut().zip(hull) {
                            *value = value.intersect(bound)?;
                        }
                    }
                }
            }
            result = Some(result.map_or(jet, |old| {
                std::array::from_fn(|a| {
                    std::array::from_fn(|b| {
                        std::array::from_fn(|k| old[a][b][k].hull(jet[a][b][k]))
                    })
                })
            }));
        }
    }
    result.ok_or_else(|| conditioning("no complete original surface knot span"))
}

pub fn evaluate_bspline_surface_jet(
    surface: &BSplineSurface,
    uv: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SurfaceEnclosure, GeometryError> {
    evaluate_nurbs_surface_jet(
        surface
            .gw_nurb()
            .ok_or_else(|| conditioning("surface has no native NURBS coefficients"))?,
        uv,
        work,
        cancel,
    )
}

/// Fold literal interval functionals into the original homogeneous coefficients
/// before basis evaluation/division. This retains source affine cancellation and
/// shares basis/denominator work across every requested row.
pub(super) fn evaluate_nurbs_surface_functionals<const N: usize>(
    surface: &Surface,
    uv: [[f64; 2]; 2],
    functionals: &[super::SurfaceFunctional; N],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[super::SurfaceFunctionalEnclosure; N], GeometryError> {
    let (p, q) = surface.degrees();
    let uk = surface.u_knots();
    let vk = surface.v_knots();
    let nu = surface.n() + 1;
    let nv = surface.m() + 1;
    validate_knots(uk, p, nu, uv[0], work, cancel)?;
    validate_knots(vk, q, nv, uv[1], work, cancel)?;
    let mut result: Option<[super::SurfaceFunctionalEnclosure; N]> = None;
    for us in p..nu {
        work.charge(1, cancel)?;
        if uk[us] == uk[us + 1] || uk[us] > uv[0][1] || uk[us + 1] < uv[0][0] {
            continue;
        }
        let ub = basis_jet(
            uk,
            p,
            us,
            [uv[0][0].max(uk[us]), uv[0][1].min(uk[us + 1])],
            1,
            work,
            cancel,
        )?;
        for vs in q..nv {
            work.charge(1, cancel)?;
            if vk[vs] == vk[vs + 1] || vk[vs] > uv[1][1] || vk[vs + 1] < uv[1][0] {
                continue;
            }
            let vb = basis_jet(
                vk,
                q,
                vs,
                [uv[1][0].max(vk[vs]), uv[1][1].min(vk[vs + 1])],
                1,
                work,
                cancel,
            )?;
            let mut numerators = [[ZERO; 3]; N];
            let mut weights = [ZERO; 3];
            let mut hull: Option<Interval> = None;
            #[expect(
                clippy::needless_range_loop,
                reason = "Functional coefficient sums share the original u/v basis indices and fixed accumulation order"
            )]
            for i in 0..=p {
                for j in 0..=q {
                    work.charge(1 + N as u64, cancel)?;
                    let c = surface
                        .cpt(us - p + i, vs - q + j)
                        .map_err(|_| conditioning("missing functional coefficient"))?;
                    if (c.w != solidcore::nurbs::math::NOW) != surface.is_rational()
                        || c.z == solidcore::nurbs::math::NOZ
                    {
                        return Err(conditioning(
                            "inconsistent native functional surface coefficients",
                        ));
                    }
                    let w = Interval::point(if surface.is_rational() { c.w } else { 1.0 });
                    let coordinates = [c.x, c.y, c.z].map(Interval::point);
                    for v in coordinates.into_iter().chain([w]) {
                        Interval::new(v.lo, v.hi)?;
                    }
                    hull = Some(hull.map_or(w, |old| old.hull(w)));
                    let basis = [
                        ub[i][0].mul(vb[j][0]),
                        ub[i][1].mul(vb[j][0]),
                        ub[i][0].mul(vb[j][1]),
                    ];
                    for d in 0..3 {
                        weights[d] = weights[d].add(basis[d].mul(w));
                    }
                    for row in 0..N {
                        let f = functionals[row];
                        let coefficient = (0..3).fold(f.offset.mul(w), |sum, k| {
                            sum.add(f.direction[k].mul(coordinates[k].sub(f.origin[k].mul(w))))
                        });
                        for d in 0..3 {
                            numerators[row][d] = numerators[row][d].add(basis[d].mul(coefficient));
                        }
                    }
                }
            }
            if !surface.is_rational() {
                weights = [Interval::point(1.0), ZERO, ZERO];
            }
            weights[0] = weights[0]
                .intersect(hull.ok_or_else(|| conditioning("empty functional control net"))?)?;
            if weights[0].contains_zero() {
                return Err(conditioning(
                    "functional rational denominator not excluded from zero",
                ));
            }
            let mut rows = [super::SurfaceFunctionalEnclosure {
                position: ZERO,
                du: ZERO,
                dv: ZERO,
            }; N];
            for row in 0..N {
                let position = numerators[row][0].div(weights[0])?;
                let du = numerators[row][1]
                    .sub(position.mul(weights[1]))
                    .div(weights[0])?;
                let dv = numerators[row][2]
                    .sub(position.mul(weights[2]))
                    .div(weights[0])?;
                for v in [position, du, dv] {
                    Interval::new(v.lo, v.hi)?;
                }
                rows[row] = super::SurfaceFunctionalEnclosure { position, du, dv };
            }
            result = Some(match result {
                None => rows,
                Some(old) => std::array::from_fn(|row| super::SurfaceFunctionalEnclosure {
                    position: old[row].position.hull(rows[row].position),
                    du: old[row].du.hull(rows[row].du),
                    dv: old[row].dv.hull(rows[row].dv),
                }),
            });
        }
    }
    result.ok_or_else(|| conditioning("no complete native functional knot span"))
}
