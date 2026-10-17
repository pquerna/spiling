// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounded error-free coefficient identities shared by native adapters and proofs.

use crate::{GeometryError, GeometryWorkBudget};
use std::sync::atomic::AtomicBool;

/// Never turns a small numerical residual into zero. Products outside the
/// exponent range in which their FMA remainder is representable are refused.
#[derive(Clone)]
pub(crate) struct ExactCoefficient {
    pub(crate) terms: [f64; 128],
    pub(crate) len: usize,
}
impl ExactCoefficient {
    pub(crate) fn point(value: f64) -> Self {
        let mut result = Self {
            terms: [0.0; 128],
            len: 0,
        };
        if value != 0.0 {
            result.terms[0] = value;
            result.len = 1;
        }
        result
    }
    pub(crate) fn sign(&self) -> i8 {
        if self.len == 0 {
            0
        } else if self.terms[self.len - 1] > 0.0 {
            1
        } else {
            -1
        }
    }
    pub(crate) fn push(&mut self, value: f64) -> Option<()> {
        if value == 0.0 {
            return Some(());
        }
        let mut carry = value;
        let mut length = 0;
        for index in 0..self.len {
            let other = self.terms[index];
            let sum = carry + other;
            if !sum.is_finite() {
                return None;
            }
            let virtual_other = sum - carry;
            let residual = (carry - (sum - virtual_other)) + (other - virtual_other);
            if residual != 0.0 {
                self.terms[length] = residual;
                length += 1;
            }
            carry = sum;
        }
        if carry != 0.0 {
            if length == self.terms.len() {
                return None;
            }
            self.terms[length] = carry;
            length += 1;
        }
        self.len = length;
        Some(())
    }
    pub(crate) fn subtract(&mut self, other: &Self) -> Option<()> {
        for term in &other.terms[..other.len] {
            self.push(-term)?;
        }
        Some(())
    }
    pub(crate) fn difference(a: f64, b: f64) -> Option<Self> {
        let mut result = Self::point(a);
        result.push(-b)?;
        Some(result)
    }
    pub(crate) fn product(
        &self,
        other: &Self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let mut result = Self::point(0.0);
        for &a in &self.terms[..self.len] {
            for &b in &other.terms[..other.len] {
                work.charge(1, cancel)?;
                let ea = ((a.abs().to_bits() >> 52) & 0x7ff) as i32;
                let eb = ((b.abs().to_bits() >> 52) & 0x7ff) as i32;
                if ea == 0 || eb == 0 || !(-900..=900).contains(&(ea + eb - 2046)) {
                    return Ok(None);
                }
                let value = a * b;
                let remainder = a.mul_add(b, -value);
                if !value.is_finite()
                    || !remainder.is_finite()
                    || result.push(remainder).is_none()
                    || result.push(value).is_none()
                {
                    return Ok(None);
                }
            }
        }
        Ok(Some(result))
    }
}
