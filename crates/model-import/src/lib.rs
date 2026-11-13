// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Syntax decoding only. Geometry owns normalization and material interpretation.
pub mod stl;
pub mod threemf;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct DecodeError(pub String);
pub type Result<T> = std::result::Result<T, DecodeError>;
pub(crate) fn invalid(message: impl Into<String>) -> DecodeError {
    DecodeError(message.into())
}
pub(crate) fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(invalid("model import cancelled"))
    } else {
        Ok(())
    }
}

/// Decoder coordinates, in the enclosing model's declared units.
#[derive(Debug, Default, Clone)]
pub struct Mesh {
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}
impl Mesh {
    pub fn validate(&self) -> Result<()> {
        if self.triangles.is_empty() || self.vertices.iter().flatten().any(|v| !v.is_finite()) {
            return Err(invalid("mesh is empty or has nonfinite coordinates"));
        }
        if self
            .triangles
            .iter()
            .flatten()
            .any(|&i| i as usize >= self.vertices.len())
        {
            return Err(invalid("mesh triangle references a missing vertex"));
        }
        Ok(())
    }
}

/// 3MF row-vector affine, stored as three coordinate columns plus translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine(pub [[f64; 3]; 4]);
impl Affine {
    pub const IDENTITY: Self = Self([[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., 0.]]);
    pub fn apply(self, p: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|a| self.0[3][a] + (0..3).map(|b| p[b] * self.0[b][a]).sum::<f64>())
    }
    /// Apply self, then parent.
    pub fn then(self, parent: Self) -> Self {
        let mut result = Self::IDENTITY;
        for b in 0..3 {
            for a in 0..3 {
                result.0[b][a] = (0..3).map(|k| self.0[b][k] * parent.0[k][a]).sum();
            }
        }
        result.0[3] = parent.apply(self.0[3]);
        result
    }
    pub fn determinant(self) -> f64 {
        let [a, b, c, _] = self.0;
        a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0])
    }
    pub fn validate(self) -> Result<Self> {
        if self.0.iter().flatten().any(|v| !v.is_finite())
            || !self.determinant().is_finite()
            || self.determinant() == 0.
        {
            Err(invalid("nonfinite or singular model transform"))
        } else {
            Ok(self)
        }
    }
}
