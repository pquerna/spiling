// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! The analytic law of the kernel's cylinder, cone, sphere and torus faces in their own (form)
//! parameters. The kernel builds those kinds on the record's NURBS form only after proving the
//! form evaluates as the analytic surface (`AnalyticSupport::build`); every trim and pcurve is in
//! the form's parameters, and so is this chart: there is no second chart.
//!
//! Each knot span of an angular direction (round the axis; a sphere's latitude, a torus's tube)
//! is a rational quadratic circular arc. Its end angles come from the control points relative to
//! the support's placement, and it is accepted only in the canonical symmetric form: end weights
//! equal and the middle weight their `cos(Δ/2)` (to rounding). Its angle is then
//! `θ(s) = θ_mid + 2 atan(tan(Δ/4) (2u − 1))`, `u` the span's own parameter. A cone's or
//! cylinder's height is affine on each degree-one span with equal weights. Any other span is not
//! read: the chart refuses and the NURBS evaluation stands. Position and derivatives are the
//! analytic law composed with those maps (Faà di Bruno, order at most three), widened by the
//! rounding difference between the analytic law and the kernel's NURB.

use super::Interval;
use super::interval::{down, sin_cos_interval, up};
use super::nurbs_eval::SurfaceTensor;
use crate::{GeometryError, GeometryWorkBudget};
use solidcore::surface::surface::Surface;
use solidcore::surface::surface::p2::SurfaceKind;
use std::sync::atomic::AtomicBool;

/// Relative agreement a canonical arc's weights and angles must show.
const CANONICAL: f64 = 1e-13;
/// The analytic law's agreement with the kernel's NURB, relative to the surface's size: both are
/// exact forms of one surface, apart only by rounding and the canonical tolerance above.
const AGREEMENT: f64 = 4e-13;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Support {
    /// Radial `r0 + h t`, axial `h`.
    Cone { r0: f64, slope: f64 },
    /// Radial `R + r cos φ`, axial `r sin φ` (`R = 0` for a sphere).
    Round { major: f64, minor: f64 },
}

/// One span of a direction: its parameter range and its law there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SpanLaw {
    /// A circular arc from `mid − Δ/2` to `mid + Δ/2` (radians).
    Arc { s: [f64; 2], mid: f64, delta: f64 },
    /// An affine height from `h[0]` to `h[1]`.
    Line { s: [f64; 2], h: [f64; 2] },
}

impl SpanLaw {
    fn range(&self) -> [f64; 2] {
        match *self {
            Self::Arc { s, .. } | Self::Line { s, .. } => s,
        }
    }

    /// The law's value and its first three derivatives in `s` over `s` (inside the span).
    pub(crate) fn jet(&self, s: Interval) -> Result<[Interval; 4], GeometryError> {
        match *self {
            Self::Line {
                s: [a, b],
                h: [h0, h1],
            } => {
                let length = Interval::point(b).sub(Interval::point(a));
                let slope = Interval::point(h1).sub(Interval::point(h0)).div(length)?;
                let value = Interval::point(h0).add(s.sub(Interval::point(a)).mul(slope));
                Ok([value, slope, Interval::point(0.0), Interval::point(0.0)])
            }
            Self::Arc {
                s: [a, b],
                mid,
                delta,
            } => {
                let length = Interval::point(b).sub(Interval::point(a));
                let k = Interval::point(2.0).div(length)?;
                // x = 2u - 1: [-1, 1] on the span, past it where an end span runs on (the same
                // rational arc's continuation, within half a span).
                let x = s.sub(Interval::point(a)).mul(k).sub(Interval::point(1.0));
                if x.lo < -2.0 - 1e-12 || x.hi > 2.0 + 1e-12 {
                    return Err(GeometryError::new(
                        spiling_contracts::geometry::GeometryErrorCode::ConditioningFailure,
                        "form chart query runs past its arc's reach",
                    ));
                }
                let t = (0.25 * delta).tan();
                let t = Interval::new(down(down(t)), up(up(t)))?;
                let arg = t.mul(x);
                // atan is monotone; one ulp each way covers libm.
                let atan = Interval::new(down(down(arg.lo.atan())), up(up(arg.hi.atan())))?;
                let value = Interval::point(mid).add(Interval::point(2.0).mul(atan));
                let d = Interval::point(1.0).add(arg.square());
                let t2 = t.square();
                let first = k.mul(Interval::point(2.0).mul(t)).div(d)?;
                let second = k
                    .square()
                    .mul(Interval::point(-4.0).mul(t2).mul(arg))
                    .div(d.square())?;
                let third =
                    k.square()
                        .mul(k)
                        .mul(
                            Interval::point(-4.0).mul(t2).mul(t).mul(
                                Interval::point(1.0).sub(Interval::point(3.0).mul(arg.square())),
                            ),
                        )
                        .div(d.square().mul(d))?;
                Ok([value, first, second, third])
            }
        }
    }
}

/// A kernel analytic face's chart: its support and each direction's span laws.
#[derive(Clone, Debug)]
pub(crate) struct FormChart {
    origin: [f64; 3],
    axes: [[f64; 3]; 3],
    support: Support,
    /// Which parameter runs round the axis (0 = u).
    sweep_axis: usize,
    sweep: Vec<SpanLaw>,
    generator: Vec<SpanLaw>,
    /// The surface's size, for the agreement term.
    size: f64,
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn wrap(angle: f64) -> f64 {
    let turn = std::f64::consts::TAU;
    let a = angle.rem_euclid(turn);
    if a > std::f64::consts::PI {
        a - turn
    } else {
        a
    }
}

fn near(a: f64, b: f64, scale: f64) -> bool {
    (a - b).abs() <= CANONICAL * scale.max(1.0)
}

/// The distinct values of a knot vector with their multiplicities.
fn runs(knots: &[f64]) -> Vec<(f64, usize)> {
    let mut out: Vec<(f64, usize)> = Vec::new();
    for &k in knots {
        match out.last_mut() {
            Some(last) if last.0 == k => last.1 += 1,
            _ => out.push((k, 1)),
        }
    }
    out
}

/// The spans of a piecewise-Bezier direction of degree `degree` over `count` control points:
/// end knots of multiplicity `degree + 1`, interior ones of `degree`. `None` otherwise.
fn bezier_spans(knots: &[f64], degree: usize, count: usize) -> Option<Vec<[f64; 2]>> {
    let r = runs(knots);
    let spans = r.len().checked_sub(1)?;
    if spans == 0 || count != degree * spans + 1 {
        return None;
    }
    for (i, &(_, m)) in r.iter().enumerate() {
        let want = if i == 0 || i == spans {
            degree + 1
        } else {
            degree
        };
        if m != want {
            return None;
        }
    }
    Some(r.windows(2).map(|w| [w[0].0, w[1].0]).collect())
}

/// A rational quadratic span's arc, from its control points' angles and its weights: its mid
/// angle and signed sweep, when it is the canonical symmetric arc (end weights equal, the middle
/// one their `cos(Δ/2)`, the middle point on the bisector); `None` for any other.
fn canonical_arc(angles: [f64; 3], weights: [f64; 3]) -> Option<(f64, f64)> {
    let [a0, a1, a2] = angles;
    let [w0, w1, w2] = weights;
    let delta = wrap(a2 - a0);
    (delta != 0.0
        && near(w0, w2, w0)
        && near(w1 / w0, (0.5 * delta).cos(), 1.0)
        && near(wrap(a1 - a0), 0.5 * delta, 1.0))
    .then_some((a0 + 0.5 * delta, delta))
}

impl FormChart {
    /// The chart of a native kernel cylinder, cone, sphere or torus (not under a STEP wrapper),
    /// when its form's every span is canonical; `None` otherwise.
    pub(crate) fn of(surface: &Surface) -> Option<Self> {
        let mut source = surface;
        for _ in 0..32 {
            match source.kind() {
                SurfaceKind::CurveBounded(b) => source = b.basis(),
                _ => break,
            }
        }
        let (placement, support) = match source.kind() {
            SurfaceKind::Cone(c) => {
                let n = c.canonical();
                let slope = n.semi_angle_deg.to_radians().tan();
                (
                    n.origin,
                    Support::Cone {
                        r0: n.radius,
                        slope,
                    },
                )
            }
            SurfaceKind::Sphere(s) => (
                *s.base().position(),
                Support::Round {
                    major: 0.0,
                    minor: s.radius(),
                },
            ),
            SurfaceKind::Torus(t) => (
                *t.base().position(),
                Support::Round {
                    major: t.major_radius(),
                    minor: t.minor_radius(),
                },
            ),
            _ => return None,
        };
        let nurb = super::analytic_eval::native_surface_nurbs(source)
            .ok()
            .flatten()?;
        let o = placement.origin();
        let (x, y, z) = (placement.x_axis(), placement.y_axis(), placement.z_axis());
        let origin = [o.x, o.y, o.z];
        let axes = [[x.x, x.y, x.z], [y.x, y.y, y.z], [z.x, z.y, z.z]];
        let counts = [nurb.n() + 1, nurb.m() + 1];
        let degrees = [nurb.p(), nurb.q()];
        let knots = [nurb.u_knots(), nurb.v_knots()];
        let rational = nurb.is_rational();
        // The Euclidean net in the support's frame, with weights.
        let mut net = vec![vec![([0.0; 3], 1.0); counts[1]]; counts[0]];
        let mut size: f64 = 0.0;
        for (i, row) in net.iter_mut().enumerate() {
            for (j, slot) in row.iter_mut().enumerate() {
                let c = nurb.cpt(i, j).ok()?;
                let w = if rational { c.w } else { 1.0 };
                if !(w.is_finite() && w > 0.0) {
                    return None;
                }
                let p = [
                    c.x / w - origin[0],
                    c.y / w - origin[1],
                    c.z / w - origin[2],
                ];
                let local = [dot(p, axes[0]), dot(p, axes[1]), dot(p, axes[2])];
                size = size.max(local.iter().fold(0.0_f64, |a, v| a.max(v.abs())));
                *slot = (local, w);
            }
        }
        if !size.is_finite() {
            return None;
        }
        let major = match support {
            Support::Round { major, .. } => major,
            Support::Cone { .. } => 0.0,
        };
        // Angle round the axis, and in the meridian plane (about the tube's or sphere's centre).
        let sweep_angle = |p: [f64; 3]| p[1].atan2(p[0]);
        let meridian_angle = |p: [f64; 3]| p[2].atan2(p[0].hypot(p[1]) - major);
        let point = |axis: usize, along: usize, other: usize| {
            if axis == 0 {
                net[along][other]
            } else {
                net[other][along]
            }
        };
        // The arcs of direction `axis`, read on every line across it that is off its centre.
        // `on`: the lines across it that lie on the surface (a meridian arc is read on the sweep's
        // even columns only: its odd ones are the meridian scaled out to the sweep's corners).
        let arcs = |axis: usize,
                    angle: &dyn Fn([f64; 3]) -> f64,
                    off: &dyn Fn([f64; 3]) -> f64,
                    on: &dyn Fn(usize) -> bool|
         -> Option<Vec<SpanLaw>> {
            if degrees[axis] != 2 {
                return None;
            }
            let spans = bezier_spans(knots[axis], 2, counts[axis])?;
            let lines = counts[1 - axis];
            let reference = (0..lines)
                .filter(|l| on(*l))
                .max_by(|&a, &b| off(point(axis, 0, a).0).total_cmp(&off(point(axis, 0, b).0)))?;
            let mut out = Vec::with_capacity(spans.len());
            for (k, s) in spans.into_iter().enumerate() {
                let (i0, i1, i2) = (2 * k, 2 * k + 1, 2 * k + 2);
                let mut law = None;
                for line in std::iter::once(reference).chain((0..lines).filter(|l| on(*l))) {
                    let (p0, w0) = point(axis, i0, line);
                    let (p1, w1) = point(axis, i1, line);
                    let (p2, w2) = point(axis, i2, line);
                    if off(p0) <= CANONICAL * size || off(p2) <= CANONICAL * size {
                        continue;
                    }
                    let (mid, delta) =
                        canonical_arc([angle(p0), angle(p1), angle(p2)], [w0, w1, w2])?;
                    match law {
                        None => law = Some(SpanLaw::Arc { s, mid, delta }),
                        Some(SpanLaw::Arc {
                            mid: m, delta: d, ..
                        }) => {
                            if !near(wrap(m - mid), 0.0, 1.0) || !near(d, delta, 1.0) {
                                return None;
                            }
                        }
                        Some(SpanLaw::Line { .. }) => return None,
                    }
                }
                out.push(law?);
            }
            // One continuous angle along the direction.
            let mut unwrapped = Vec::with_capacity(out.len());
            let mut end: Option<f64> = None;
            for law in out {
                let SpanLaw::Arc { s, mid, delta } = law else {
                    return None;
                };
                let start = mid - 0.5 * delta;
                let mid = match end {
                    Some(e) => mid + (e - start - wrap(e - start)),
                    None => mid,
                };
                end = Some(mid + 0.5 * delta);
                unwrapped.push(SpanLaw::Arc { s, mid, delta });
            }
            Some(unwrapped)
        };
        let radial = |p: [f64; 3]| p[0].hypot(p[1]);
        let tube = |p: [f64; 3]| (p[0].hypot(p[1]) - major).hypot(p[2]);
        for sweep_axis in 0..2 {
            let Some(sweep) = arcs(sweep_axis, &sweep_angle, &radial, &|_| true) else {
                continue;
            };
            let g = 1 - sweep_axis;
            let generator = match support {
                Support::Round { .. } => match arcs(g, &meridian_angle, &tube, &|l| l % 2 == 0) {
                    Some(laws) => laws,
                    None => continue,
                },
                Support::Cone { .. } => {
                    if degrees[g] != 1 {
                        continue;
                    }
                    let Some(spans) = bezier_spans(knots[g], 1, counts[g]) else {
                        continue;
                    };
                    let line = 0;
                    let mut laws = Vec::with_capacity(spans.len());
                    let mut ok = true;
                    for (k, s) in spans.into_iter().enumerate() {
                        let (p0, w0) = point(g, k, line);
                        let (p1, w1) = point(g, k + 1, line);
                        if !near(w0, w1, w0) {
                            ok = false;
                            break;
                        }
                        laws.push(SpanLaw::Line {
                            s,
                            h: [p0[2], p1[2]],
                        });
                    }
                    // Every line along the generator has the same heights.
                    for i in 0..counts[sweep_axis] {
                        for (k, law) in laws.iter().enumerate() {
                            let SpanLaw::Line { h, .. } = law else {
                                continue;
                            };
                            let (a, wa) = point(g, k, i);
                            let (b, wb) = point(g, k + 1, i);
                            if !near(a[2], h[0], size)
                                || !near(b[2], h[1], size)
                                || !near(wa, wb, wa)
                            {
                                ok = false;
                            }
                        }
                    }
                    if !ok {
                        continue;
                    }
                    laws
                }
            };
            return Some(Self {
                origin,
                axes,
                support,
                sweep_axis,
                sweep,
                generator,
                size,
            });
        }
        None
    }

    /// The direction laws over `range`, one per span it meets, each with its clipped range.
    fn pieces(laws: &[SpanLaw], range: [f64; 2]) -> Vec<(SpanLaw, [f64; 2])> {
        let last = laws.len().saturating_sub(1);
        laws.iter()
            .enumerate()
            .filter_map(|(i, law)| {
                let [mut a, mut b] = law.range();
                // The end spans run on past the form, onto the chart's reach.
                if i == 0 {
                    a = f64::NEG_INFINITY;
                }
                if i == last {
                    b = f64::INFINITY;
                }
                let lo = range[0].max(a);
                let hi = range[1].min(b);
                (lo <= hi).then_some((*law, [lo, hi]))
            })
            .collect()
    }

    /// The parameter box the chart's law reaches, in (u, v): the form's range, each line
    /// direction run on by its length each way and each arc direction by half its end span
    /// (the same arc continued, well short of a turn).
    pub(crate) fn reach(&self) -> [[f64; 2]; 2] {
        let span = |laws: &[SpanLaw]| -> [f64; 2] {
            let (Some(first), Some(last)) = (laws.first(), laws.last()) else {
                return [0.0; 2];
            };
            let ([a, a1], [b0, b]) = (first.range(), last.range());
            match (first, last) {
                (SpanLaw::Line { .. }, _) => {
                    let length = b - a;
                    [a - length, b + length]
                }
                _ => [a - 0.5 * (a1 - a), b + 0.5 * (b - b0)],
            }
        };
        let (sw, ge) = (span(&self.sweep), span(&self.generator));
        if self.sweep_axis == 0 {
            [sw, ge]
        } else {
            [ge, sw]
        }
    }

    /// The analytic angle (or height) intervals over a parameter box: `[sweep, generator]`.
    pub(crate) fn angles(&self, uv: [[f64; 2]; 2]) -> Result<Option<[Interval; 2]>, GeometryError> {
        let ranges = if self.sweep_axis == 0 {
            uv
        } else {
            [uv[1], uv[0]]
        };
        let mut out = [None::<Interval>; 2];
        for (slot, (laws, range)) in out
            .iter_mut()
            .zip([(&self.sweep, ranges[0]), (&self.generator, ranges[1])])
        {
            let pieces = Self::pieces(laws, range);
            if pieces.is_empty() {
                return Ok(None);
            }
            for (law, [lo, hi]) in pieces {
                let v = law.jet(Interval::new(lo, hi)?)?[0];
                *slot = Some(slot.map_or(v, |s: Interval| s.hull(v)));
            }
        }
        Ok(match out {
            [Some(a), Some(b)] => Some([a, b]),
            _ => None,
        })
    }

    /// The analytic law's derivatives in (θ, generator) up to order three each: `[a][b]` is
    /// `∂θ^a ∂g^b`, in the support's frame.
    fn law(&self, theta: Interval, g: Interval) -> [[[Interval; 3]; 4]; 4] {
        let zero = Interval::point(0.0);
        let (s, c) = sin_cos_interval(theta);
        // θ-derivatives of (cos θ, sin θ).
        let e = [[c, s], [s.neg(), c], [c.neg(), s.neg()], [s, c.neg()]];
        // Radial and axial generator functions and their derivatives.
        let (radial, axial) = match self.support {
            Support::Cone { r0, slope } => {
                let t = Interval::point(slope);
                (
                    [Interval::point(r0).add(g.mul(t)), t, zero, zero],
                    [g, Interval::point(1.0), zero, zero],
                )
            }
            Support::Round { major, minor } => {
                let (sg, cg) = sin_cos_interval(g);
                let r = Interval::point(minor);
                (
                    [
                        Interval::point(major).add(r.mul(cg)),
                        r.mul(sg).neg(),
                        r.mul(cg).neg(),
                        r.mul(sg),
                    ],
                    [r.mul(sg), r.mul(cg), r.mul(sg).neg(), r.mul(cg).neg()],
                )
            }
        };
        let mut out = [[[zero; 3]; 4]; 4];
        for (a, row) in out.iter_mut().enumerate() {
            for (b, slot) in row.iter_mut().enumerate() {
                for k in 0..3 {
                    let x = Interval::point(self.axes[0][k]);
                    let y = Interval::point(self.axes[1][k]);
                    let zk = Interval::point(self.axes[2][k]);
                    let dir = e[a][0].mul(x).add(e[a][1].mul(y));
                    let mut v = radial[b].mul(dir);
                    if a == 0 {
                        v = v.add(axial[b].mul(zk));
                    }
                    if a == 0 && b == 0 {
                        v = v.add(Interval::point(self.origin[k]));
                    }
                    slot[k] = v;
                }
            }
        }
        out
    }

    /// The tensor `[i][j] = ∂u^i ∂v^j` over a box inside the chart, to order `order` (≤ 3).
    #[allow(clippy::needless_range_loop)] // Faà di Bruno: the orders index the Bell tables
    pub(crate) fn tensor(
        &self,
        uv: [[f64; 2]; 2],
        order: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<SurfaceTensor>, GeometryError> {
        let ranges = if self.sweep_axis == 0 {
            uv
        } else {
            [uv[1], uv[0]]
        };
        let sweeps = Self::pieces(&self.sweep, ranges[0]);
        let gens = Self::pieces(&self.generator, ranges[1]);
        if sweeps.is_empty() || gens.is_empty() {
            return Ok(None);
        }
        let zero = Interval::point(0.0);
        let mut hull: Option<SurfaceTensor> = None;
        for (sl, sr) in &sweeps {
            let tj = sl.jet(Interval::new(sr[0], sr[1])?)?;
            for (gl, gr) in &gens {
                work.charge(1, cancel)?;
                let gj = gl.jet(Interval::new(gr[0], gr[1])?)?;
                let f = self.law(tj[0], gj[0]);
                // Bell polynomials: bell[m][a] for m-th derivative with a outer derivatives.
                let bell = |j: &[Interval; 4]| -> [[Interval; 4]; 4] {
                    let mut b = [[zero; 4]; 4];
                    b[0][0] = Interval::point(1.0);
                    b[1][1] = j[1];
                    b[2][1] = j[2];
                    b[2][2] = j[1].square();
                    b[3][1] = j[3];
                    b[3][2] = Interval::point(3.0).mul(j[1]).mul(j[2]);
                    b[3][3] = j[1].square().mul(j[1]);
                    b
                };
                let (bs, bg) = (bell(&tj), bell(&gj));
                let mut t: SurfaceTensor = [[[zero; 3]; 4]; 4];
                for m in 0..=order {
                    for n in 0..=order - m {
                        for k in 0..3 {
                            let mut v = zero;
                            for a in 0..=m {
                                for b in 0..=n {
                                    if bs[m][a] == zero || bg[n][b] == zero {
                                        continue;
                                    }
                                    v = v.add(f[a][b][k].mul(bs[m][a]).mul(bg[n][b]));
                                }
                            }
                            let (i, j) = if self.sweep_axis == 0 { (m, n) } else { (n, m) };
                            t[i][j][k] = v;
                        }
                    }
                }
                hull = Some(match hull {
                    None => t,
                    Some(h) => {
                        let mut out = h;
                        for i in 0..4 {
                            for j in 0..4 {
                                for k in 0..3 {
                                    out[i][j][k] = h[i][j][k].hull(t[i][j][k]);
                                }
                            }
                        }
                        out
                    }
                });
            }
        }
        let Some(mut t) = hull else { return Ok(None) };
        // The analytic law against the kernel's NURB: rounding apart.
        let reach =
            AGREEMENT * (self.size + self.origin.iter().fold(0.0_f64, |a, v| a.max(v.abs())));
        for (i, row) in t.iter_mut().enumerate() {
            for (j, entry) in row.iter_mut().enumerate() {
                if i + j > order {
                    *entry = [zero; 3];
                    continue;
                }
                for v in entry.iter_mut() {
                    // The position carries the absolute term; a derivative only its relative
                    // one, so a structural zero (a vertical wall's dz) stays exact, as the form's.
                    let by = if i + j == 0 { reach } else { 0.0 } + AGREEMENT * v.abs_upper();
                    if by == 0.0 {
                        continue;
                    }
                    *v = Interval::new(down(v.lo - by), up(v.hi + by))?;
                }
            }
        }
        Ok(Some(t))
    }

    /// The sign of `du x dv` against the support's outward `q`: each direction's turning sense,
    /// which parameter runs round the axis, and the frame's handedness.
    pub(crate) fn orientation(&self) -> f64 {
        let sense = |laws: &[SpanLaw]| match laws.first() {
            Some(SpanLaw::Arc { delta, .. }) => delta.signum(),
            Some(SpanLaw::Line { h, .. }) => (h[1] - h[0]).signum(),
            None => 1.0,
        };
        let [x, y, z] = self.axes;
        let cross = [
            x[1] * y[2] - x[2] * y[1],
            x[2] * y[0] - x[0] * y[2],
            x[0] * y[1] - x[1] * y[0],
        ];
        let hand = dot(cross, z).signum();
        let swap = if self.sweep_axis == 0 { 1.0 } else { -1.0 };
        sense(&self.sweep) * sense(&self.generator) * swap * hand
    }

    /// The meridian angle of a whole row `v` (the form's second parameter) where that runs along
    /// the meridian and `v` is an end of its range; `None` otherwise.
    pub(crate) fn row_latitude(&self, v: f64) -> Option<f64> {
        if self.sweep_axis != 0 {
            return None;
        }
        let (first, last) = (self.generator.first()?, self.generator.last()?);
        let at = |law: &SpanLaw, end: usize| match *law {
            SpanLaw::Arc { s, mid, delta } => {
                (s[end] == v).then_some(mid + if end == 0 { -0.5 } else { 0.5 } * delta)
            }
            SpanLaw::Line { .. } => None,
        };
        at(first, 0).or_else(|| at(last, 1))
    }

    /// Which form parameter runs round the axis (0 = u) and the generator's span ends: each end's
    /// parameter and its meridian angle (or height).
    #[cfg(test)]
    pub(crate) fn generator_ends(&self) -> (usize, Vec<(f64, f64)>) {
        let mut out = Vec::new();
        for law in &self.generator {
            match *law {
                SpanLaw::Arc { s, mid, delta } => {
                    out.push((s[0], mid - 0.5 * delta));
                    out.push((s[1], mid + 0.5 * delta));
                }
                SpanLaw::Line { s, h } => {
                    out.push((s[0], h[0]));
                    out.push((s[1], h[1]));
                }
            }
        }
        out.dedup_by(|a, b| a.0 == b.0);
        (self.sweep_axis, out)
    }

    /// Whether two charts are one law of `(u, v)`: the same support (numbers equal), the same
    /// direction round the axis, and in each direction every span of both a piece of one
    /// equal-span law (each arc the same sweep over the same length, its mid angle affine in its
    /// start with one offset, to a whole turn; each line one affine height).
    pub(crate) fn same_law(&self, other: &Self) -> bool {
        if self.support != other.support
            || self.origin != other.origin
            || self.axes != other.axes
            || self.sweep_axis != other.sweep_axis
        {
            return false;
        }
        let one = |laws: Vec<&SpanLaw>| -> bool {
            let Some(first) = laws.first() else {
                return false;
            };
            match **first {
                SpanLaw::Arc { s, mid, delta } => {
                    let length = s[1] - s[0];
                    let offset = mid - delta * s[0] / length;
                    laws.iter().all(|law| match **law {
                        SpanLaw::Arc {
                            s: t,
                            mid: m,
                            delta: d,
                        } => {
                            near(t[1] - t[0], length, length)
                                && near(d, delta, 1.0)
                                && near(wrap(m - d * t[0] / (t[1] - t[0]) - offset), 0.0, 1.0)
                        }
                        SpanLaw::Line { .. } => false,
                    })
                }
                SpanLaw::Line { s, h } => {
                    let slope = (h[1] - h[0]) / (s[1] - s[0]);
                    let at0 = h[0] - slope * s[0];
                    laws.iter().all(|law| match **law {
                        SpanLaw::Line { s: t, h: g } => {
                            let k = (g[1] - g[0]) / (t[1] - t[0]);
                            near(k, slope, slope.abs()) && near(g[0] - k * t[0], at0, self.size)
                        }
                        SpanLaw::Arc { .. } => false,
                    })
                }
            }
        };
        one(self.sweep.iter().chain(&other.sweep).collect())
            && one(self.generator.iter().chain(&other.generator).collect())
    }

    /// The angle the face's form turns about the axis over its whole sweep.
    #[cfg(test)]
    pub(crate) fn sweep_turn(&self) -> f64 {
        self.sweep
            .iter()
            .map(|law| match law {
                SpanLaw::Arc { delta, .. } => *delta,
                SpanLaw::Line { .. } => 0.0,
            })
            .sum()
    }

    /// A sphere's support: centre, axes and radius, for pole and normal laws.
    pub(crate) fn sphere(&self) -> Option<([f64; 3], [[f64; 3]; 3], f64)> {
        match self.support {
            Support::Round { major: 0.0, minor } => Some((self.origin, self.axes, minor)),
            _ => None,
        }
    }
}

/// The analytic law of the kernel's conic edge (an ellipse or circle, `conic_on_form`) in its
/// own (form) parameter: each span of its form a canonical rational arc of the conic, read by its
/// eccentric angles in the conic's frame; `None` for any other span.
#[derive(Clone, Debug)]
pub(crate) struct ConicChart {
    origin: [f64; 3],
    /// The conic's axes scaled by their radii.
    axes: [[f64; 3]; 2],
    spans: Vec<SpanLaw>,
    size: f64,
}

impl ConicChart {
    pub(crate) fn of(curve: &solidcore::curve::curve::Curve) -> Option<Self> {
        use solidcore::curve::curve::p2::CurveKind;
        let CurveKind::Ellipse(e) = curve.kind() else {
            return None;
        };
        let c = e.canonical();
        let (o, x, y) = (
            c.position.origin(),
            c.position.x_axis(),
            c.position.y_axis(),
        );
        let (rx, ry) = (c.radius_x, c.radius_y);
        if !(rx > 0.0 && ry > 0.0 && rx.is_finite() && ry.is_finite()) {
            return None;
        }
        let origin = [o.x, o.y, o.z];
        let (x, y) = ([x.x, x.y, x.z], [y.x, y.y, y.z]);
        let nurb = e.base().gw_nurb()?;
        if nurb.degree() != 2 {
            return None;
        }
        let cpts = nurb.cpts();
        let spans = bezier_spans(nurb.knots(), 2, cpts.len())?;
        let rational = nurb.is_rational();
        let mut net = Vec::with_capacity(cpts.len());
        let mut size: f64 = rx.max(ry);
        for p in cpts {
            let w = if rational { p.w } else { 1.0 };
            if !(w.is_finite() && w > 0.0) {
                return None;
            }
            let d = [
                p.x / w - origin[0],
                p.y / w - origin[1],
                p.z / w - origin[2],
            ];
            size = size.max(d.iter().fold(0.0_f64, |a, v| a.max(v.abs())));
            // The eccentric angle of the point in the conic's frame.
            net.push(((dot(d, y) / ry).atan2(dot(d, x) / rx), w));
        }
        let mut laws = Vec::with_capacity(spans.len());
        let mut end: Option<f64> = None;
        for (k, s) in spans.into_iter().enumerate() {
            let (&(a0, w0), &(a1, w1), &(a2, w2)) =
                (net.get(2 * k)?, net.get(2 * k + 1)?, net.get(2 * k + 2)?);
            let (mid, delta) = canonical_arc([a0, a1, a2], [w0, w1, w2])?;
            let start = mid - 0.5 * delta;
            let mid = match end {
                Some(e) => mid + (e - start - wrap(e - start)),
                None => mid,
            };
            end = Some(mid + 0.5 * delta);
            laws.push(SpanLaw::Arc { s, mid, delta });
        }
        let axes = [x.map(|v| v * rx), y.map(|v| v * ry)];
        Some(Self {
            origin,
            axes,
            spans: laws,
            size: size + origin.iter().fold(0.0_f64, |a, v| a.max(v.abs())),
        })
    }

    /// The parameter range the law reaches: the form's, each end span run on by half its length.
    pub(crate) fn reach(&self) -> [f64; 2] {
        let (Some(first), Some(last)) = (self.spans.first(), self.spans.last()) else {
            return [0.0; 2];
        };
        let ([a, a1], [b0, b]) = (first.range(), last.range());
        [a - 0.5 * (a1 - a), b + 0.5 * (b - b0)]
    }

    /// The jet `[d0, d1, d2, d3]` (position and derivatives) over `range` in the form parameter.
    pub(crate) fn jet(
        &self,
        range: [f64; 2],
        order: usize,
    ) -> Result<Option<[[Interval; 3]; 4]>, GeometryError> {
        let zero = Interval::point(0.0);
        let mut hull: Option<[[Interval; 3]; 4]> = None;
        for (law, [lo, hi]) in FormChart::pieces(&self.spans, range) {
            let j = law.jet(Interval::new(lo, hi)?)?;
            let (s, c) = sin_cos_interval(j[0]);
            let e = [[c, s], [s.neg(), c], [c.neg(), s.neg()], [s, c.neg()]];
            let bell = [
                [Interval::point(1.0), zero, zero, zero],
                [zero, j[1], zero, zero],
                [zero, j[2], j[1].square(), zero],
                [
                    zero,
                    j[3],
                    Interval::point(3.0).mul(j[1]).mul(j[2]),
                    j[1].square().mul(j[1]),
                ],
            ];
            let mut out = [[zero; 3]; 4];
            for (m, slot) in out.iter_mut().enumerate().take(order + 1) {
                for (k, v) in slot.iter_mut().enumerate() {
                    let mut sum = if m == 0 {
                        Interval::point(self.origin[k])
                    } else {
                        zero
                    };
                    for (a, b) in bell[m].iter().enumerate() {
                        if *b == zero {
                            continue;
                        }
                        let f = e[a][0]
                            .mul(Interval::point(self.axes[0][k]))
                            .add(e[a][1].mul(Interval::point(self.axes[1][k])));
                        sum = sum.add(f.mul(*b));
                    }
                    *v = sum;
                }
            }
            hull = Some(match hull {
                None => out,
                Some(h) => {
                    std::array::from_fn(|m| std::array::from_fn(|k| h[m][k].hull(out[m][k])))
                }
            });
        }
        let Some(mut out) = hull else { return Ok(None) };
        for (m, row) in out.iter_mut().enumerate() {
            for v in row.iter_mut() {
                let by =
                    if m == 0 { AGREEMENT * self.size } else { 0.0 } + AGREEMENT * v.abs_upper();
                if by > 0.0 {
                    *v = Interval::new(down(v.lo - by), up(v.hi + by))?;
                }
            }
        }
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(s: [f64; 2], from: f64, delta: f64) -> SpanLaw {
        SpanLaw::Arc {
            s,
            mid: from + 0.5 * delta,
            delta,
        }
    }

    /// The canonical arc: its angle, at the span's ends and middle, and its derivatives against
    /// central differences.
    #[test]
    #[allow(clippy::needless_range_loop)]
    fn an_arc_law_and_its_derivatives_match_finite_differences() {
        for (s, from, delta) in [
            ([0.0, 1.0], 0.0, std::f64::consts::FRAC_PI_2),
            ([2.0, 2.7], 1.0, -0.4),
        ] {
            let law = arc(s, from, delta);
            let at = |x: f64| law.jet(Interval::point(x)).unwrap();
            assert!(at(s[0])[0].contains(from) || (at(s[0])[0].mid() - from).abs() < 1e-14);
            assert!((at(s[1])[0].mid() - (from + delta)).abs() < 1e-14);
            assert!((at(0.5 * (s[0] + s[1]))[0].mid() - (from + 0.5 * delta)).abs() < 1e-14);
            let h = 1e-5;
            for x in [
                s[0] + 0.1 * (s[1] - s[0]),
                0.5 * (s[0] + s[1]),
                s[1] - 0.2 * (s[1] - s[0]),
            ] {
                for d in 0..3 {
                    let fd = (at(x + h)[d].mid() - at(x - h)[d].mid()) / (2.0 * h);
                    let exact = at(x)[d + 1].mid();
                    assert!(
                        (fd - exact).abs() <= 1e-5 * exact.abs().max(1.0),
                        "{d}: {fd} vs {exact}"
                    );
                }
            }
            // An interval encloses its points.
            let box_ = law.jet(Interval::new(s[0], s[1]).unwrap()).unwrap();
            for x in [s[0], 0.3 * s[0] + 0.7 * s[1], s[1]] {
                for d in 0..4 {
                    assert!(box_[d].contains(at(x)[d].mid()));
                }
            }
        }
    }

    /// The rational quadratic arc's own points are the law's: the canonical form's parameter.
    #[test]
    fn an_arc_law_is_the_canonical_rational_arcs_parameter() {
        let (from, delta) = (0.3_f64, 1.1_f64);
        let law = arc([0.0, 1.0], from, delta);
        let w = (0.5 * delta).cos();
        let p = |a: f64| [a.cos(), a.sin()];
        let (p0, p2) = (p(from), p(from + delta));
        let m = p(from + 0.5 * delta);
        let p1 = [m[0] / w, m[1] / w];
        for u in [0.0, 0.2, 0.5, 0.77, 1.0] {
            let b = [(1.0 - u) * (1.0 - u), 2.0 * u * (1.0 - u) * w, u * u];
            let den = b[0] + b[1] + b[2];
            let x = (b[0] * p0[0] + b[1] * p1[0] + b[2] * p2[0]) / den;
            let y = (b[0] * p0[1] + b[1] * p1[1] + b[2] * p2[1]) / den;
            let theta = law.jet(Interval::point(u)).unwrap()[0].mid();
            assert!((y.atan2(x) - theta).abs() < 1e-14, "{u}");
        }
    }

    /// A span is read only in the canonical symmetric form: a gauged arc (end weights unequal),
    /// a middle weight off `cos(Δ/2)` or a middle point off the bisector is refused.
    #[test]
    fn a_non_canonical_span_is_refused() {
        let q = std::f64::consts::FRAC_PI_2;
        let c = (0.5 * q).cos();
        assert!(canonical_arc([0.0, 0.5 * q, q], [1.0, c, 1.0]).is_some());
        assert!(canonical_arc([0.0, 0.5 * q, q], [2.0, 2.0 * c, 2.0]).is_some());
        assert!(canonical_arc([0.0, 0.5 * q, q], [1.0, c, 2.0]).is_none());
        assert!(canonical_arc([0.0, 0.5 * q, q], [1.0, 0.8, 1.0]).is_none());
        assert!(canonical_arc([0.0, 0.4 * q, q], [1.0, c, 1.0]).is_none());
        assert!(canonical_arc([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]).is_none());
        // Clockwise is a canonical arc too, of negative sweep.
        assert_eq!(
            canonical_arc([q, 0.5 * q, 0.0], [1.0, c, 1.0]).map(|(_, d)| d),
            Some(-q)
        );
    }

    #[test]
    fn spans_must_be_piecewise_bezier() {
        assert_eq!(
            bezier_spans(&[0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 2.0], 2, 5),
            Some(vec![[0.0, 1.0], [1.0, 2.0]])
        );
        assert_eq!(
            bezier_spans(&[0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0], 2, 4),
            None
        );
        assert_eq!(
            bezier_spans(&[0.0, 0.0, 1.0, 1.0], 1, 2),
            Some(vec![[0.0, 1.0]])
        );
    }
}
#[cfg(test)]
#[test]
fn the_chart_is_the_kernel_forms_law_on_cylinders_cones_spheres_and_tori() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(u64::MAX / 4);
    let mut faces = 0;
    for name in [
        "native-parity-blind-pocket.step",
        "native-parity-cone-frustum.step",
        "native-parity-sphere-belt.step",
        "native-parity-full-sphere.step",
        "native-parity-ring-torus.step",
    ] {
        let d = crate::analysis::test_source::recipe(name);
        for f in d.face_ids() {
            let s = d.face_surface(*f).unwrap();
            let Some(chart) = FormChart::of(s) else {
                assert!(
                    !crate::parametric::analytic_eval::native_analytic_support(s),
                    "{name}: an analytic face without its chart"
                );
                continue;
            };
            faces += 1;
            let nurb = super::analytic_eval::native_surface_nurbs(s)
                .unwrap()
                .unwrap();
            let (u, v) = (nurb.u_knots(), nurb.v_knots());
            let (u0, u1) = (u[nurb.p()], u[nurb.n() + 1]);
            let (v0, v1) = (v[nurb.q()], v[nurb.m() + 1]);
            for a in [0.0, 0.13, 0.5, 0.71, 1.0] {
                for b in [0.0, 0.37, 0.5, 1.0] {
                    let uv = [u0 + a * (u1 - u0), v0 + b * (v1 - v0)];
                    let at = [[uv[0]; 2], [uv[1]; 2]];
                    let ours = chart.tensor(at, 2, &mut work, &cancel).unwrap().unwrap();
                    let kernel = super::nurbs_eval::evaluate_nurbs_surface_tensor(
                        nurb, at, 2, &mut work, &cancel,
                    )
                    .unwrap();
                    for (i, j) in [(0, 0), (1, 0), (0, 1), (2, 0), (1, 1), (0, 2)] {
                        for k in 0..3 {
                            let (x, y) = (ours[i][j][k].mid(), kernel[i][j][k].mid());
                            let scale = 1.0 + y.abs() + chart.size;
                            assert!(
                                (x - y).abs() <= 1e-8 * scale,
                                "{name} {uv:?} d{i}{j}[{k}]: {x} vs {y}"
                            );
                        }
                    }
                }
            }
        }
    }
    assert!(faces >= 20, "{faces} charts");
}

/// A sphere's poles in its form chart: `du` vanishes there, and the normal is the algebraic
/// field's, along the axis.
#[cfg(test)]
#[test]
fn a_spheres_pole_has_its_axial_normal_in_the_form_chart() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(u64::MAX / 4);
    let d = crate::analysis::test_source::recipe("native-parity-full-sphere.step");
    let mut poles = 0;
    for f in d.face_ids() {
        let s = d.face_surface(*f).unwrap();
        let Some(chart) = FormChart::of(s) else {
            continue;
        };
        let Some((centre, axes, radius)) = chart.sphere() else {
            continue;
        };
        let nurb = super::analytic_eval::native_surface_nurbs(s)
            .unwrap()
            .unwrap();
        let (u, v) = (nurb.u_knots(), nurb.v_knots());
        for vv in [v[nurb.q()], v[nurb.m() + 1]] {
            let uu = 0.5 * (u[nurb.p()] + u[nurb.n() + 1]);
            let at = [[uu; 2], [vv; 2]];
            let p = chart.tensor(at, 1, &mut work, &cancel).unwrap().unwrap();
            let height = (0..3)
                .map(|k| (p[0][0][k].mid() - centre[k]) * axes[2][k])
                .sum::<f64>();
            if (height.abs() - radius).abs() > 1e-9 * radius {
                continue;
            }
            poles += 1;
            let du = (0..3).map(|k| p[1][0][k].mid().abs()).fold(0.0, f64::max);
            assert!(du <= 1e-9 * radius, "{du}");
            let n =
                crate::parametric::evaluate_analytic_normal_enclosure(s, at, &mut work, &cancel)
                    .unwrap();
            let along = (0..3).map(|k| n[k].mid() * axes[2][k]).sum::<f64>();
            assert!((along.abs() - 1.0).abs() < 1e-9, "{n:?}");
        }
    }
    assert!(poles >= 2, "{poles} poles");
}

/// The conic chart is the kernel form's own law on every conic edge of the recipes: position
/// and derivatives to order three agree with the NURB's at points along each edge.
#[cfg(test)]
#[test]
fn the_conic_chart_is_the_kernel_forms_law_on_circle_and_ellipse_edges() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(u64::MAX / 4);
    let mut edges = 0;
    for name in [
        "native-parity-blind-pocket.step",
        "native-parity-cone-frustum.step",
        "native-parity-sphere-belt.step",
        "native-parity-ring-torus.step",
        "cylinder.step",
    ] {
        let d = crate::analysis::test_source::recipe(name);
        for &edge in d.brep().edge_list() {
            let Some(curve) = d.edge_curve(edge) else {
                continue;
            };
            let Some(chart) = ConicChart::of(curve) else {
                continue;
            };
            edges += 1;
            let nurb = super::curve_prepared::native_curve_nurbs(curve)
                .unwrap()
                .unwrap();
            let k = nurb.knots();
            let (a, b) = (k[nurb.degree()], k[nurb.cpts().len()]);
            for f in [0.0, 0.17, 0.5, 0.83, 1.0] {
                let t = a + f * (b - a);
                let ours = chart.jet([t, t], 3).unwrap().unwrap();
                let form = super::curve_prepared::validated_nurbs_jet(
                    curve,
                    nurb,
                    [t, t],
                    3,
                    &mut work,
                    &cancel,
                )
                .unwrap();
                for m in 0..4 {
                    for c in 0..3 {
                        let (x, y) = (ours[m][c].mid(), form[m][c].mid());
                        assert!(
                            (x - y).abs() <= 1e-8 * (1.0 + y.abs() + chart.size),
                            "{name} t={t} d{m}[{c}]: {x} vs {y}"
                        );
                    }
                }
            }
        }
    }
    assert!(edges >= 8, "{edges} conic charts");
}
