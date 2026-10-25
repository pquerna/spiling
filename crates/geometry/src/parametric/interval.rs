// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! IEEE-754 outward-rounded interval arithmetic, vector types, and work debiting.

use crate::{GeometryError, GeometryWorkBudget, check_cancel};
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

#[inline]
pub fn down(value: f64) -> f64 {
    if value.is_nan() || value == f64::NEG_INFINITY {
        return value;
    }
    if value == 0.0 {
        return -f64::from_bits(1);
    }
    f64::from_bits(if value > 0.0 {
        value.to_bits() - 1
    } else {
        value.to_bits() + 1
    })
}

#[inline]
pub fn up(value: f64) -> f64 {
    if value.is_nan() || value == f64::INFINITY {
        return value;
    }
    if value == 0.0 {
        return f64::from_bits(1);
    }
    f64::from_bits(if value > 0.0 {
        value.to_bits() + 1
    } else {
        value.to_bits() - 1
    })
}

#[inline]
pub fn next_after(from: f64, toward: f64) -> f64 {
    if from.is_nan() || toward.is_nan() {
        return f64::NAN;
    }
    if from == toward {
        return toward;
    }
    if toward > from { up(from) } else { down(from) }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
}

impl Interval {
    pub fn new(lo: f64, hi: f64) -> Result<Self, GeometryError> {
        if !lo.is_finite() || !hi.is_finite() || lo > hi {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                format!("invalid finite parameter interval: [{lo}, {hi}]"),
            ));
        }
        Ok(Self { lo, hi })
    }

    #[inline]
    pub const fn point(value: f64) -> Self {
        Self {
            lo: value,
            hi: value,
        }
    }

    #[inline(always)]
    pub fn new_unchecked(lo: f64, hi: f64) -> Self {
        Self { lo, hi }
    }

    #[inline]
    pub fn width(self) -> f64 {
        up(self.hi - self.lo)
    }

    #[inline]
    pub fn diameter(self) -> f64 {
        self.width()
    }

    #[inline]
    pub fn mid(self) -> f64 {
        self.lo * 0.5 + self.hi * 0.5
    }

    #[inline]
    pub fn abs_upper(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }

    #[inline]
    pub fn abs_lower(self) -> f64 {
        if self.contains_zero() {
            0.0
        } else {
            self.lo.abs().min(self.hi.abs())
        }
    }

    #[inline]
    pub fn contains(self, x: f64) -> bool {
        self.lo <= x && x <= self.hi
    }

    #[inline]
    pub fn contains_interval(self, other: Self) -> bool {
        self.lo <= other.lo && other.hi <= self.hi
    }

    #[inline]
    pub fn contains_zero(self) -> bool {
        self.lo <= 0.0 && self.hi >= 0.0
    }

    #[inline]
    pub fn is_point(self) -> bool {
        self.lo == self.hi
    }

    #[inline]
    pub fn hull(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let lo = self.lo.max(other.lo);
        let hi = self.hi.min(other.hi);
        if lo <= hi {
            Some(Self { lo, hi })
        } else {
            None
        }
    }

    pub fn intersect(self, other: Self) -> Result<Self, GeometryError> {
        self.intersection(other).ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "intervals do not overlap",
            )
        })
    }

    pub fn add(self, other: Self) -> Self {
        if self.lo == 0.0 && self.hi == 0.0 {
            return other;
        }
        if other.lo == 0.0 && other.hi == 0.0 {
            return self;
        }
        if self.lo == self.hi && other.lo == other.hi && self.lo == -other.lo {
            return Self::point(0.0);
        }
        if self.lo == self.hi && other.lo == other.hi {
            let sum = self.lo + other.lo;
            if sum.is_finite() {
                // Error-free TwoSum uses only addition/subtraction.
                let virtual_other = sum - self.lo;
                let residual = (self.lo - (sum - virtual_other)) + (other.lo - virtual_other);
                if residual == 0.0 {
                    return Self::point(sum);
                }
            }
        }
        Self {
            lo: if self.lo == 0.0 {
                other.lo
            } else if other.lo == 0.0 {
                self.lo
            } else {
                down(self.lo + other.lo)
            },
            hi: if self.hi == 0.0 {
                other.hi
            } else if other.hi == 0.0 {
                self.hi
            } else {
                up(self.hi + other.hi)
            },
        }
    }

    #[inline]
    pub fn neg(self) -> Self {
        Self {
            lo: -self.hi,
            hi: -self.lo,
        }
    }

    #[inline]
    pub fn sub(self, other: Self) -> Self {
        self.add(other.neg())
    }

    pub fn mul(self, other: Self) -> Self {
        if (self.lo == 0.0 && self.hi == 0.0) || (other.lo == 0.0 && other.hi == 0.0) {
            return Self::point(0.0);
        }
        if self.lo == 1.0 && self.hi == 1.0 {
            return other;
        }
        if other.lo == 1.0 && other.hi == 1.0 {
            return self;
        }
        if self.lo == -1.0 && self.hi == -1.0 {
            return other.neg();
        }
        if other.lo == -1.0 && other.hi == -1.0 {
            return self.neg();
        }

        let p1 = self.lo * other.lo;
        let p2 = self.lo * other.hi;
        let p3 = self.hi * other.lo;
        let p4 = self.hi * other.hi;

        if p1.is_nan() || p2.is_nan() || p3.is_nan() || p4.is_nan() {
            return Self {
                lo: f64::NEG_INFINITY,
                hi: f64::INFINITY,
            };
        }

        // Product signs are exact even when a rounded corner product
        // underflows. Do not widen an attained zero across its proved sign.
        let mut lo = down(p1.min(p2).min(p3).min(p4));
        let mut hi = up(p1.max(p2).max(p3).max(p4));
        if (self.lo >= 0.0 && other.lo >= 0.0) || (self.hi <= 0.0 && other.hi <= 0.0) {
            lo = lo.max(0.0);
        }
        if (self.lo >= 0.0 && other.hi <= 0.0) || (self.hi <= 0.0 && other.lo >= 0.0) {
            hi = hi.min(0.0);
        }
        Self { lo, hi }
    }

    pub fn reciprocal(self) -> Result<Self, GeometryError> {
        if self.contains_zero() {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "interval containing zero has unbounded reciprocal",
            ));
        }
        Ok(Self {
            lo: down(1.0 / self.hi),
            hi: up(1.0 / self.lo),
        })
    }

    pub fn div(self, other: Self) -> Result<Self, GeometryError> {
        if other.contains_zero() {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "whole-interval rational denominator cannot be separated from zero",
            ));
        }
        if other.lo == 1.0 && other.hi == 1.0 {
            return Ok(self);
        }
        if other.lo == -1.0 && other.hi == -1.0 {
            return Ok(self.neg());
        }
        if self.lo == 0.0 && self.hi == 0.0 {
            return Ok(Self::point(0.0));
        }
        if self.lo == self.hi && other.lo == other.hi && self.lo == other.lo {
            return Ok(Self::point(1.0));
        }
        // Divide endpoints directly: forming 1/denominator first can overflow
        // even when a ratio of two subnormal homogeneous coefficients is finite.
        let quotients = [
            self.lo / other.lo,
            self.lo / other.hi,
            self.hi / other.lo,
            self.hi / other.hi,
        ];
        if quotients.into_iter().any(f64::is_nan) {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "indeterminate interval quotient",
            ));
        }
        let lo = quotients.into_iter().fold(f64::INFINITY, f64::min);
        let hi = quotients.into_iter().fold(f64::NEG_INFINITY, f64::max);
        Ok(Self {
            lo: down(lo),
            hi: up(hi),
        })
    }

    pub fn square(self) -> Self {
        if self.lo == self.hi {
            if self.lo == 0.0 {
                return Self::point(0.0);
            }
            if self.lo == 1.0 || self.lo == -1.0 {
                return Self::point(1.0);
            }
        }
        if self.contains_zero() {
            let max_abs = self.abs_upper();
            Self {
                lo: 0.0,
                hi: up(max_abs * max_abs),
            }
        } else {
            let sq1 = self.lo * self.lo;
            let sq2 = self.hi * self.hi;
            Self {
                lo: down(sq1.min(sq2)).max(0.0),
                hi: up(sq1.max(sq2)),
            }
        }
    }

    pub fn sqrt(self) -> Result<Self, GeometryError> {
        if self.hi < 0.0 || !self.hi.is_finite() {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "invalid square-root enclosure: interval is negative",
            ));
        }
        if self.lo == self.hi && (self.lo == 0.0 || self.lo == 1.0) {
            return Ok(self);
        }
        let lo = if self.lo <= 0.0 {
            0.0
        } else {
            down(self.lo.sqrt()).max(0.0)
        };
        let hi = up(self.hi.sqrt());
        Ok(Self { lo, hi })
    }
}

impl std::ops::Add for Interval {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self::Output {
        self.add(rhs)
    }
}

impl std::ops::Sub for Interval {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self::Output {
        self.sub(rhs)
    }
}

impl std::ops::Mul for Interval {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self::Output {
        self.mul(rhs)
    }
}

impl std::ops::Neg for Interval {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self::Output {
        self.neg()
    }
}

/// Rigorous sine/cosine bounds. Reduction uses an enclosure of mathematical pi;
/// a bounded Taylor remainder replaces assumptions about platform libm ulps.
pub fn sin_cos_interval(angle: Interval) -> (Interval, Interval) {
    let full = Interval { lo: -1.0, hi: 1.0 };
    if !angle.lo.is_finite()
        || !angle.hi.is_finite()
        || angle.lo > angle.hi
        || angle.abs_upper() > 1_000_000.0
        || angle.width() > 1.0
    {
        return (full, full);
    }
    let n = (angle.mid() / std::f64::consts::FRAC_PI_2).round() as i64;
    let half_pi = Interval {
        lo: down(std::f64::consts::PI),
        hi: up(std::f64::consts::PI),
    }
    .mul(Interval::point(0.5));
    let x = angle.sub(Interval::point(n as f64).mul(half_pi));
    if x.abs_upper() > 2.0 {
        return (full, full);
    }
    let minus_square = x.square().neg();
    let mut cos_term = Interval::point(1.0);
    let mut sin_term = x;
    let mut cosine = cos_term;
    let mut sine = sin_term;
    for k in 1..=16 {
        // Small integer denominators are exact binary64 coefficients.
        cos_term = cos_term.mul(minus_square).mul(
            Interval::point(((2 * k - 1) * (2 * k)) as f64)
                .reciprocal()
                .unwrap(),
        );
        sin_term = sin_term.mul(minus_square).mul(
            Interval::point(((2 * k) * (2 * k + 1)) as f64)
                .reciprocal()
                .unwrap(),
        );
        cosine = cosine.add(cos_term);
        sine = sine.add(sin_term);
    }
    let cos_error = cos_term
        .mul(x.square())
        .mul(Interval::point((33 * 34) as f64).reciprocal().unwrap())
        .abs_upper();
    let sin_error = sin_term
        .mul(x.square())
        .mul(Interval::point((34 * 35) as f64).reciprocal().unwrap())
        .abs_upper();
    cosine = cosine.add(Interval {
        lo: -cos_error,
        hi: cos_error,
    });
    sine = sine.add(Interval {
        lo: -sin_error,
        hi: sin_error,
    });
    cosine = Interval {
        lo: cosine.lo.max(-1.0),
        hi: cosine.hi.min(1.0),
    };
    sine = Interval {
        lo: sine.lo.max(-1.0),
        hi: sine.hi.min(1.0),
    };
    match n.rem_euclid(4) {
        0 => (sine, cosine),
        1 => (cosine, sine.neg()),
        2 => (sine.neg(), cosine.neg()),
        _ => (cosine.neg(), sine),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntervalVec2 {
    pub u: Interval,
    pub v: Interval,
}

impl IntervalVec2 {
    #[inline]
    pub fn new(u: Interval, v: Interval) -> Self {
        Self { u, v }
    }

    #[inline]
    pub fn point(u: f64, v: f64) -> Self {
        Self {
            u: Interval::point(u),
            v: Interval::point(v),
        }
    }

    #[inline]
    pub fn from_array(arr: [Interval; 2]) -> Self {
        Self {
            u: arr[0],
            v: arr[1],
        }
    }

    #[inline]
    pub fn to_array(self) -> [Interval; 2] {
        [self.u, self.v]
    }

    #[inline]
    pub fn add(self, other: Self) -> Self {
        Self {
            u: self.u.add(other.u),
            v: self.v.add(other.v),
        }
    }

    #[inline]
    pub fn sub(self, other: Self) -> Self {
        Self {
            u: self.u.sub(other.u),
            v: self.v.sub(other.v),
        }
    }

    #[inline]
    pub fn scale(self, factor: Interval) -> Self {
        Self {
            u: self.u.mul(factor),
            v: self.v.mul(factor),
        }
    }

    #[inline]
    pub fn dot(self, other: Self) -> Interval {
        self.u.mul(other.u).add(self.v.mul(other.v))
    }

    #[inline]
    pub fn norm_squared(self) -> Interval {
        let sq = self.u.square().add(self.v.square());
        Interval {
            lo: sq.lo.max(0.0),
            hi: sq.hi,
        }
    }

    #[inline]
    pub fn norm(self) -> Result<Interval, GeometryError> {
        self.norm_squared().sqrt()
    }

    #[inline]
    pub fn norm_upper(self) -> Result<f64, GeometryError> {
        Ok(self.norm()?.hi)
    }

    #[inline]
    pub fn hull(self, other: Self) -> Self {
        Self {
            u: self.u.hull(other.u),
            v: self.v.hull(other.v),
        }
    }

    #[inline]
    pub fn width(self) -> [f64; 2] {
        [self.u.width(), self.v.width()]
    }
}

impl From<[Interval; 2]> for IntervalVec2 {
    #[inline]
    fn from(arr: [Interval; 2]) -> Self {
        Self::from_array(arr)
    }
}

impl From<IntervalVec2> for [Interval; 2] {
    #[inline]
    fn from(v: IntervalVec2) -> Self {
        v.to_array()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntervalVec3 {
    pub x: Interval,
    pub y: Interval,
    pub z: Interval,
}

impl IntervalVec3 {
    #[inline]
    pub fn new(x: Interval, y: Interval, z: Interval) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub fn point(x: f64, y: f64, z: f64) -> Self {
        Self {
            x: Interval::point(x),
            y: Interval::point(y),
            z: Interval::point(z),
        }
    }

    #[inline]
    pub fn from_point(p: [f64; 3]) -> Self {
        Self::point(p[0], p[1], p[2])
    }

    #[inline]
    pub fn from_array(arr: [Interval; 3]) -> Self {
        Self {
            x: arr[0],
            y: arr[1],
            z: arr[2],
        }
    }

    #[inline]
    pub fn to_array(self) -> [Interval; 3] {
        [self.x, self.y, self.z]
    }

    #[inline]
    pub fn add(self, other: Self) -> Self {
        Self {
            x: self.x.add(other.x),
            y: self.y.add(other.y),
            z: self.z.add(other.z),
        }
    }

    #[inline]
    pub fn sub(self, other: Self) -> Self {
        Self {
            x: self.x.sub(other.x),
            y: self.y.sub(other.y),
            z: self.z.sub(other.z),
        }
    }

    #[inline]
    pub fn scale(self, factor: Interval) -> Self {
        Self {
            x: self.x.mul(factor),
            y: self.y.mul(factor),
            z: self.z.mul(factor),
        }
    }

    #[inline]
    pub fn dot(self, other: Self) -> Interval {
        self.x
            .mul(other.x)
            .add(self.y.mul(other.y))
            .add(self.z.mul(other.z))
    }

    #[inline]
    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y.mul(other.z).sub(self.z.mul(other.y)),
            y: self.z.mul(other.x).sub(self.x.mul(other.z)),
            z: self.x.mul(other.y).sub(self.y.mul(other.x)),
        }
    }

    #[inline]
    pub fn norm_squared(self) -> Interval {
        let sq = self.x.square().add(self.y.square()).add(self.z.square());
        // Sums of squares are analytically non-negative; outward rounding of zero
        // could produce a negative subnormal lower bound.
        Interval {
            lo: sq.lo.max(0.0),
            hi: sq.hi,
        }
    }

    #[inline]
    pub fn norm(self) -> Result<Interval, GeometryError> {
        self.norm_squared().sqrt()
    }

    #[inline]
    pub fn norm_upper(self) -> Result<f64, GeometryError> {
        Ok(self.norm()?.hi)
    }

    #[inline]
    pub fn hull(self, other: Self) -> Self {
        Self {
            x: self.x.hull(other.x),
            y: self.y.hull(other.y),
            z: self.z.hull(other.z),
        }
    }

    #[inline]
    pub fn width(self) -> [f64; 3] {
        [self.x.width(), self.y.width(), self.z.width()]
    }
}

impl From<[Interval; 3]> for IntervalVec3 {
    #[inline]
    fn from(arr: [Interval; 3]) -> Self {
        Self::from_array(arr)
    }
}

impl From<IntervalVec3> for [Interval; 3] {
    #[inline]
    fn from(v: IntervalVec3) -> Self {
        v.to_array()
    }
}

#[inline]
pub fn debit_native_work(
    work: &GeometryWorkBudget,
    consumed: usize,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    work.charge(consumed as u64, &AtomicBool::new(false))?;
    check_cancel(cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_float_stepping() {
        assert!(down(0.0) < 0.0);
        assert!(up(0.0) > 0.0);
        assert!(down(1.0) < 1.0);
        assert!(up(1.0) > 1.0);
        assert_eq!(next_after(1.0, 2.0), up(1.0));
        assert_eq!(next_after(1.0, 0.0), down(1.0));
        assert_eq!(next_after(1.0, 1.0), 1.0);
    }

    #[test]
    fn test_interval_outward_rounding() {
        let a = Interval::new(1.0, 2.0).unwrap();
        let b = Interval::new(3.0, 4.0).unwrap();
        let sum = a.add(b);
        assert!(sum.lo <= 4.0);
        assert!(sum.hi >= 6.0);

        let sub = a.sub(b);
        assert!(sub.lo <= -3.0);
        assert!(sub.hi >= -1.0);

        let mul = a.mul(b);
        assert!(mul.lo <= 3.0);
        assert!(mul.hi >= 8.0);
    }

    #[test]
    fn inward_transport_preserves_the_closed_domain_endpoint() {
        let anchor = Interval::point(1.570796326795);
        let offset = Interval::new(-4.0e-7, 0.0).unwrap();
        let across = Interval::new(0.0, 1.0).unwrap();
        let moved = anchor.add(across.mul(offset));
        assert_eq!(moved.hi, anchor.hi);
        assert!(moved.lo <= anchor.lo - 4.0e-7);
        let opposite = anchor.neg().add(across.mul(offset.neg()));
        assert_eq!(opposite.lo, -anchor.hi);
        assert!(opposite.hi >= -anchor.lo + 4.0e-7);
    }

    #[test]
    fn signed_underflow_encloses_nonzero_products_without_crossing_zero() {
        let tiny = f64::from_bits(1);
        let positive = Interval::point(tiny).mul(Interval::point(0.5));
        assert_eq!(positive.lo, 0.0);
        assert!(positive.hi > 0.0);
        let negative = Interval::point(-tiny).mul(Interval::point(0.5));
        assert!(negative.lo < 0.0);
        assert_eq!(negative.hi, 0.0);
        let crossing = Interval::new(-tiny, tiny)
            .unwrap()
            .mul(Interval::point(0.5));
        assert!(crossing.lo < 0.0 && crossing.hi > 0.0);
    }

    #[test]
    fn test_twosum_exact_point_cancellation() {
        let p1 = Interval::point(1e16);
        let p2 = Interval::point(-1e16);
        let sum = p1.add(p2);
        assert_eq!(sum.lo, 0.0);
        assert_eq!(sum.hi, 0.0);
    }

    #[test]
    fn test_square_contains_zero() {
        let i = Interval::new(-3.0, 4.0).unwrap();
        let sq = i.square();
        assert_eq!(sq.lo, 0.0);
        assert!(sq.hi >= 16.0);
    }

    #[test]
    fn test_sqrt_bounds() {
        let i = Interval::new(4.0, 9.0).unwrap();
        let s = i.sqrt().unwrap();
        assert!(s.lo <= 2.0);
        assert!(s.hi >= 3.0);

        let neg = Interval::new(-4.0, -1.0).unwrap();
        assert!(neg.sqrt().is_err());
    }

    #[test]
    fn test_reciprocal_and_div() {
        let i = Interval::new(2.0, 4.0).unwrap();
        let recip = i.reciprocal().unwrap();
        assert!(recip.lo <= 0.25);
        assert!(recip.hi >= 0.5);

        let zero = Interval::new(-1.0, 1.0).unwrap();
        assert!(zero.reciprocal().is_err());
        assert!(i.div(zero).is_err());
    }

    #[test]
    fn test_intersection() {
        let a = Interval::new(1.0, 3.0).unwrap();
        let b = Interval::new(2.0, 4.0).unwrap();
        let c = a.intersection(b).unwrap();
        assert_eq!(c.lo, 2.0);
        assert_eq!(c.hi, 3.0);

        let d = Interval::new(5.0, 6.0).unwrap();
        assert!(a.intersection(d).is_none());
    }

    #[test]
    fn test_sin_cos_interval() {
        use std::f64::consts::PI;
        let quarter = Interval::new(0.0, 0.5 * PI).unwrap();
        let (sin_q, cos_q) = sin_cos_interval(quarter);
        assert!(sin_q.lo <= 0.0 && sin_q.hi >= 1.0);
        assert!(cos_q.lo <= 0.0 && cos_q.hi >= 1.0);

        let full = Interval::new(0.0, 2.5 * PI).unwrap();
        let (sin_f, cos_f) = sin_cos_interval(full);
        assert_eq!(sin_f.lo, -1.0);
        assert_eq!(sin_f.hi, 1.0);
        assert_eq!(cos_f.lo, -1.0);
        assert_eq!(cos_f.hi, 1.0);
    }

    #[test]
    fn test_vector_cross_and_norm() {
        let v1 = IntervalVec3::point(1.0, 0.0, 0.0);
        let v2 = IntervalVec3::point(0.0, 1.0, 0.0);
        let cross = v1.cross(v2);
        assert!(cross.x.contains(0.0));
        assert!(cross.y.contains(0.0));
        assert!(cross.z.contains(1.0));

        let norm = cross.norm().unwrap();
        assert!(norm.contains(1.0));

        let zero_vec = IntervalVec3::point(0.0, 0.0, 0.0);
        let zero_norm = zero_vec.norm().unwrap();
        assert_eq!(zero_norm.lo, 0.0);
        assert!(zero_norm.hi >= 0.0);
    }
}
