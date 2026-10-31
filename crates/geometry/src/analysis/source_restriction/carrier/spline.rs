// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A common tensor basis, proved from the stored knots, not a STEP proxy.
use crate::exact_coefficient::ExactCoefficient as Exact;
use crate::parametric::{Interval, analytic_eval::native_surface_nurbs};
use crate::{GeometryError, GeometryWorkBudget};
use solidcore::surface::surface::Surface as NativeSurface;
use solidcraft_step::geom::Surface;
use std::sync::atomic::AtomicBool;

const ZERO: Interval = Interval::point(0.0);
macro_rules! available {
    ($value:expr) => {
        match $value {
            Some(value) => value,
            None => return Ok(None),
        }
    };
}

fn enclosure(value: &Exact) -> Interval {
    value.terms[..value.len]
        .iter()
        .fold(ZERO, |sum, &term| sum.add(Interval::point(term)))
}
fn magnitude(value: Interval) -> f64 {
    value.lo.abs().max(value.hi.abs())
}

/// Verify the entire expanded vector without materializing an expanded copy.
/// Positive endpoint differences preserve axis direction and every multiplicity.
fn axis(
    values: &[f64],
    multiplicities: &[usize],
    native: &[f64],
    degree: usize,
    count: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<([Interval; 2], [f64; 2])>, GeometryError> {
    if values.is_empty()
        || values.len() != multiplicities.len()
        || count <= degree
        || count.checked_add(degree).and_then(|n| n.checked_add(1)) != Some(native.len())
    {
        return Ok(None);
    }
    let mut expanded = 0usize;
    let mut domain = [0.0; 2];
    for (index, (&value, &multiplicity)) in values.iter().zip(multiplicities).enumerate() {
        work.charge(1, cancel)?;
        if !value.is_finite()
            || multiplicity == 0
            || multiplicity > degree + 1
            || (index > 0 && values[index - 1] >= value)
        {
            return Ok(None);
        }
        let end = available!(expanded.checked_add(multiplicity));
        if expanded <= degree && degree < end {
            domain[0] = value;
        }
        if expanded <= count && count < end {
            domain[1] = value;
        }
        expanded = end;
    }
    if expanded != native.len() || domain[0] >= domain[1] {
        return Ok(None);
    }
    crate::parametric::nurbs_eval::validate_knots(
        native,
        degree,
        count,
        [native[degree], native[count]],
        work,
        cancel,
    )?;
    let sd = available!(Exact::difference(domain[1], domain[0]));
    let nd = available!(Exact::difference(native[count], native[degree]));
    let mut index = 0;
    for (&source, &multiplicity) in values.iter().zip(multiplicities) {
        for _ in 0..multiplicity {
            work.charge(1, cancel)?;
            let a = available!(Exact::difference(source, domain[0]));
            let b = available!(Exact::difference(native[index], native[degree]));
            let mut identity = available!(a.product(&nd, work, cancel)?);
            let right = available!(b.product(&sd, work, cancel)?);
            available!(identity.subtract(&right));
            if identity.sign() != 0 {
                return Ok(None);
            }
            index += 1;
        }
    }
    let scale = enclosure(&sd).div(enclosure(&nd))?;
    let offset = Interval::point(domain[0]).sub(Interval::point(native[degree]).mul(scale));
    if ![scale.lo, scale.hi, offset.lo, offset.hi]
        .iter()
        .all(|x| x.is_finite())
    {
        return Ok(None);
    }
    Ok(Some(([offset, scale], domain)))
}

pub(super) fn try_associate(
    original: &Surface,
    native: &NativeSurface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<super::CarrierRecord>, GeometryError> {
    work.charge(1, cancel)?;
    let Surface::BSpline(source) = original else {
        return extrusion(original, native, work, cancel);
    };
    let Some(native) = native_surface_nurbs(native)? else {
        return Ok(None);
    };
    let nu = source.points.len();
    let nv = source.points.first().map_or(0, Vec::len);
    if nu == 0 || nv == 0 || native.degrees() != (source.u_degree, source.v_degree) {
        return Ok(None);
    }
    let indices = native.indices();
    if indices.n.checked_add(1) != Some(nu) || indices.m.checked_add(1) != Some(nv) {
        return Ok(None);
    }
    if let Some(weights) = &source.weights
        && weights.len() != nu
    {
        return Ok(None);
    }
    // The bridge flattens source rows with v fastest; create_canonical keeps
    // that ordering. It does not swap or reverse either spline axis.
    let (u, ud) = available!(axis(
        &source.u_knots,
        &source.u_multiplicities,
        native.u_knots(),
        source.u_degree,
        nu,
        work,
        cancel
    )?);
    let (v, vd) = available!(axis(
        &source.v_knots,
        &source.v_multiplicities,
        native.v_knots(),
        source.v_degree,
        nv,
        work,
        cancel
    )?);
    let origin = source.points[0][0];
    if !origin.iter().all(|x| x.is_finite()) {
        return Ok(None);
    }
    let sw0 = source.weights.as_ref().map_or(1.0, |w| {
        w.first()
            .and_then(|r| r.first())
            .copied()
            .unwrap_or(f64::NAN)
    });
    let anchor = match native.cpt(0, 0) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let nw0 = if native.is_rational() { anchor.w } else { 1.0 };
    if !sw0.is_finite() || sw0 <= 0.0 || !nw0.is_finite() || nw0 <= 0.0 {
        return Ok(None);
    }
    let mut min_weight = f64::INFINITY;
    let mut max_h = 0.0_f64;
    let mut max_w = 0.0_f64;
    let mut radius = 0.0_f64;
    let mut literal = true;
    for (i, row) in source.points.iter().enumerate() {
        work.charge(1, cancel)?;
        if row.len() != nv || source.weights.as_ref().is_some_and(|w| w[i].len() != nv) {
            return Ok(None);
        }
        for (j, point) in row.iter().enumerate() {
            work.charge(1, cancel)?;
            let sw = source.weights.as_ref().map_or(1.0, |w| w[i][j]);
            let c = match native.cpt(i, j) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
            if (c.w != solidcore::nurbs::math::NOW) != native.is_rational()
                || c.z == solidcore::nurbs::math::NOZ
            {
                return Ok(None);
            }
            let nw = if native.is_rational() { c.w } else { 1.0 };
            let h = [c.x, c.y, c.z];
            if !point
                .iter()
                .chain(h.iter())
                .chain([sw, nw].iter())
                .all(|x| x.is_finite())
                || sw <= 0.0
                || nw <= 0.0
            {
                return Ok(None);
            }
            min_weight = min_weight.min(nw);
            let mut dw = available!(Exact::point(nw).product(&Exact::point(sw0), work, cancel)?);
            let right = available!(Exact::point(sw).product(&Exact::point(nw0), work, cancel)?);
            available!(dw.subtract(&right));
            literal &= dw.sign() == 0;
            max_w = max_w.max(magnitude(enclosure(&dw)));
            let mut h_norm = ZERO;
            let mut r_norm = ZERO;
            for k in 0..3 {
                work.charge(1, cancel)?;
                let centered = available!(Exact::difference(point[k], origin[k]));
                r_norm = r_norm.add(Interval::point(magnitude(enclosure(&centered))));
                let ow =
                    available!(Exact::point(origin[k]).product(&Exact::point(nw), work, cancel)?);
                let mut nh = Exact::point(h[k]);
                available!(nh.subtract(&ow));
                let mut dh = available!(nh.product(&Exact::point(sw0), work, cancel)?);
                let sh = available!(centered.product(&Exact::point(sw), work, cancel)?);
                let scaled = available!(sh.product(&Exact::point(nw0), work, cancel)?);
                available!(dh.subtract(&scaled));
                literal &= dh.sign() == 0;
                h_norm = h_norm.add(Interval::point(magnitude(enclosure(&dh))));
            }
            max_h = max_h.max(h_norm.hi);
            radius = radius.max(r_norm.hi);
        }
    }
    // L1 norms dominate Euclidean norms. The positive common tensor basis
    // has sum one; source rational points stay inside its Cartesian cage.
    let error = if literal {
        0.0
    } else {
        Interval::point(max_h)
            .add(Interval::point(radius).mul(Interval::point(max_w)))
            .div(Interval::point(min_weight).mul(Interval::point(sw0)))?
            .hi
    };
    if !error.is_finite() || error < 0.0 {
        return Ok(None);
    }
    Ok(Some(super::CarrierRecord {
        from_native: [[u[0], u[1], ZERO], [v[0], ZERO, v[1]]],
        original_domain: [Some(ud), Some(vd)],
        error_coefficients: [error, 0.0, 0.0, 0.0],
        literal,
    }))
}

/// A borrowed profile and two native tensor rows prove the entire extrusion
/// law. No profile is materialized, and no angular/rational chart is inferred.
fn extrusion(
    original: &Surface,
    native: &NativeSurface,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<super::CarrierRecord>, GeometryError> {
    let Surface::Extrusion {
        profile,
        direction,
        magnitude: axial,
    } = original
    else {
        return Ok(None);
    };
    let solidcraft_step::geom::Curve::BSpline(profile) = profile.as_ref() else {
        return Ok(None);
    };
    let Some(native) = native_surface_nurbs(native)? else {
        return Ok(None);
    };
    let count = profile.points.len();
    if count == 0
        || native.degrees() != (profile.degree, 1)
        || native.indices().n.checked_add(1) != Some(count)
        || native.indices().m != 1
        || profile.weights.as_ref().is_some_and(|w| w.len() != count)
        || !direction.iter().chain([axial]).all(|x| x.is_finite())
    {
        return Ok(None);
    }
    let (u, ud) = available!(axis(
        &profile.knots,
        &profile.multiplicities,
        native.u_knots(),
        profile.degree,
        count,
        work,
        cancel
    )?);
    // Source extrusion v is the literal profile displacement multiplier.
    let vk = native.v_knots();
    if vk.len() != 4
        || !vk.iter().all(|x| x.is_finite())
        || vk[0] != vk[1]
        || vk[2] != vk[3]
        || vk[0] >= vk[3]
    {
        return Ok(None);
    }
    let v = [ZERO, Interval::point(1.0)];
    let anchor = match native.cpt(0, 0) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let nw0 = if native.is_rational() { anchor.w } else { 1.0 };
    let sw0 = profile.weights.as_ref().map_or(1.0, |w| w[0]);
    if !nw0.is_finite() || nw0 <= 0.0 || !sw0.is_finite() || sw0 <= 0.0 {
        return Ok(None);
    }
    let mut errors = [0.0_f64; 2];
    let mut literal = true;
    for i in 0..count {
        work.charge(1, cancel)?;
        let sw = profile.weights.as_ref().map_or(1.0, |w| w[i]);
        if !sw.is_finite() || sw <= 0.0 || !profile.points[i].iter().all(|x| x.is_finite()) {
            return Ok(None);
        }
        let mut residuals = [[ZERO; 3]; 2];
        for j in 0..2 {
            let c = match native.cpt(i, j) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
            if (c.w != solidcore::nurbs::math::NOW) != native.is_rational()
                || c.z == solidcore::nurbs::math::NOZ
            {
                return Ok(None);
            }
            let nw = if native.is_rational() { c.w } else { 1.0 };
            if !nw.is_finite() || nw <= 0.0 || ![c.x, c.y, c.z].iter().all(|x| x.is_finite()) {
                return Ok(None);
            }
            let mut weight =
                available!(Exact::point(nw).product(&Exact::point(sw0), work, cancel)?);
            let source_weight =
                available!(Exact::point(sw).product(&Exact::point(nw0), work, cancel)?);
            available!(weight.subtract(&source_weight));
            // Equal rational denominator is required for extrapolation in the
            // unbounded source extrusion axis, not merely a cage-distance bound.
            if weight.sign() != 0 {
                return Ok(None);
            }
            for (k, h) in [c.x, c.y, c.z].into_iter().enumerate() {
                work.charge(1, cancel)?;
                let mut point = Exact::point(profile.points[i][k]);
                let displacement = available!(Exact::point(direction[k]).product(
                    &Exact::point(*axial),
                    work,
                    cancel
                )?);
                let displacement =
                    available!(displacement.product(&Exact::point(vk[j * 2]), work, cancel)?);
                for &term in &displacement.terms[..displacement.len] {
                    available!(point.push(term));
                }
                let weighted = available!(point.product(&Exact::point(nw), work, cancel)?);
                let mut delta = Exact::point(h);
                available!(delta.subtract(&weighted));
                literal &= delta.sign() == 0;
                residuals[j][k] = enclosure(&delta).div(Interval::point(nw))?;
            }
        }
        let span = Interval::point(vk[2]).sub(Interval::point(vk[0]));
        let slopes: [Interval; 3] = std::array::from_fn(|k| residuals[1][k].sub(residuals[0][k]));
        let mut base = ZERO;
        let mut slope = ZERO;
        for k in 0..3 {
            let delta = slopes[k].div(span)?;
            slope = slope.add(Interval::point(magnitude(delta)));
            base = base.add(Interval::point(magnitude(
                residuals[0][k].sub(delta.mul(Interval::point(vk[0]))),
            )));
        }
        errors[0] = errors[0].max(base.hi);
        errors[1] = errors[1].max(slope.hi);
    }
    Ok(Some(super::CarrierRecord {
        from_native: [[u[0], u[1], ZERO], [v[0], ZERO, v[1]]],
        // Convex profile-basis bounds require source u inside its actual knot
        // domain; the affine source-v coefficient law permits unbounded v.
        original_domain: [Some(ud), None],
        error_coefficients: if literal {
            [0.0; 4]
        } else {
            [errors[0], 0.0, errors[1], 0.0]
        },
        literal,
    }))
}
