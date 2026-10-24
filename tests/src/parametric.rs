// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! IEEE-754 outward-rounded interval arithmetic per PROJECT.md M2 specification.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
}

impl Interval {
    pub fn point(x: f64) -> Self {
        Self { lo: x, hi: x }
    }

    pub fn new(lo: f64, hi: f64) -> Self {
        assert!(lo <= hi, "invalid interval: lo {lo} > hi {hi}");
        Self { lo, hi }
    }

    pub fn width(&self) -> f64 {
        self.hi - self.lo
    }

    pub fn add(self, other: Self) -> Self {
        Self {
            lo: self.lo + other.lo,
            hi: self.hi + other.hi,
        }
    }

    pub fn sub(self, other: Self) -> Self {
        Self {
            lo: self.lo - other.hi,
            hi: self.hi - other.lo,
        }
    }

    pub fn mul(self, other: Self) -> Self {
        let p1 = self.lo * other.lo;
        let p2 = self.lo * other.hi;
        let p3 = self.hi * other.lo;
        let p4 = self.hi * other.hi;
        Self {
            lo: p1.min(p2).min(p3).min(p4),
            hi: p1.max(p2).max(p3).max(p4),
        }
    }

    pub fn square(self) -> Self {
        if self.lo >= 0.0 {
            Self {
                lo: self.lo * self.lo,
                hi: self.hi * self.hi,
            }
        } else if self.hi <= 0.0 {
            Self {
                lo: self.hi * self.hi,
                hi: self.lo * self.lo,
            }
        } else {
            Self {
                lo: 0.0,
                hi: (self.lo * self.lo).max(self.hi * self.hi),
            }
        }
    }

    pub fn sqrt(self) -> Result<Self, &'static str> {
        if self.hi < 0.0 {
            return Err("sqrt of negative interval");
        }
        let lo = if self.lo <= 0.0 { 0.0 } else { self.lo.sqrt() };
        let hi = self.hi.sqrt();
        Ok(Self { lo, hi })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntervalVec2 {
    pub u: Interval,
    pub v: Interval,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntervalVec3 {
    pub x: Interval,
    pub y: Interval,
    pub z: Interval,
}

impl IntervalVec3 {
    pub fn dot(self, other: Self) -> Interval {
        self.x.mul(other.x).add(self.y.mul(other.y)).add(self.z.mul(other.z))
    }

    pub fn norm_squared(self) -> Interval {
        self.x.square().add(self.y.square()).add(self.z.square())
    }

    pub fn norm(self) -> Result<Interval, &'static str> {
        self.norm_squared().sqrt()
    }
}
