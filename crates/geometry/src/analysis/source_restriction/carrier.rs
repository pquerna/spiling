// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native-to-original coefficient gauges. This is neither an original PCURVE
//! declaration nor material authority: selected trim/branch and whole source
//! correspondence remain independently proved by the owning trim preparation.
use crate::parametric::Interval;
use crate::{GeometryError, GeometryWorkBudget};
use solidcore::surface::surface::Surface;
use solidcraft_step::geom::Surface as OriginalSurface;
use spiling_contracts::geometry::GeometryErrorCode;
use std::sync::atomic::AtomicBool;

mod analytic;
mod spline;

pub(super) struct CarrierRecord {
    pub(super) from_native: [[Interval; 3]; 2],
    pub(super) original_domain: [Option<[f64; 2]>; 2],
    /// Upper bounds c0 + cU |u| + cV |v| + cUV |uv|, derived from actual
    /// position-law coefficients, not any requested/admitted tolerance.
    pub(super) error_coefficients: [f64; 4],
    pub(super) literal: bool,
}

pub(crate) struct SourceCarrierAssociation {
    record: CarrierRecord,
}
fn unavailable(message: &'static str) -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
impl SourceCarrierAssociation {
    pub(crate) fn new(
        original: &OriginalSurface,
        native: &Surface,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        let record = match analytic::try_associate(original, native, work, cancel)? {
            Some(record) => record,
            None => spline::try_associate(original, native, work, cancel)?.ok_or_else(|| {
                unavailable(
                    "original/native carriers have no checked coefficient gauge association",
                )
            })?,
        };
        for coordinate in record.from_native.iter().flatten() {
            Interval::new(coordinate.lo, coordinate.hi)?;
        }
        for coefficient in record.error_coefficients {
            if !coefficient.is_finite() || coefficient < 0.0 {
                return Err(unavailable(
                    "carrier coefficient deviation has no finite nonnegative bound",
                ));
            }
        }
        let determinant = record.from_native[0][1]
            .mul(record.from_native[1][2])
            .sub(record.from_native[0][2].mul(record.from_native[1][1]));
        if determinant.contains_zero() {
            return Err(unavailable(
                "native/original carrier gauge has no proved nonsingular affine map",
            ));
        }
        for range in record.original_domain.into_iter().flatten() {
            Interval::new(range[0], range[1])?;
        }
        Ok(Self { record })
    }
    pub(crate) fn source_from_native(&self) -> [[Interval; 3]; 2] {
        self.record.from_native
    }
    /// Exact coefficient identity under the proved gauge, not equality of
    /// declared labels, domains, active branches or rounded query samples.
    #[cfg(test)]
    pub(crate) fn is_literal(&self) -> bool {
        self.record.literal
    }
    pub(crate) fn original_natural_domain(&self) -> [Option<[f64; 2]>; 2] {
        self.record.original_domain
    }
    /// Bound corresponding valid native/original coefficient parameters in this
    /// source UV box. This does not establish native preimage coverage of every
    /// box point or qualify any original trim branch.
    pub(crate) fn position_error_mm(
        &self,
        source_uv_box: [[f64; 2]; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        work.charge(1, cancel)?;
        let u = Interval::new(source_uv_box[0][0], source_uv_box[0][1])?;
        let v = Interval::new(source_uv_box[1][0], source_uv_box[1][1])?;
        // The spline coefficient/cage theorem requires its actual natural
        // domain; analytic laws have no such restriction on this bound.
        if source_uv_box
            .iter()
            .zip(self.record.original_domain)
            .any(|(query, natural)| {
                natural.is_some_and(|natural| query[0] < natural[0] || query[1] > natural[1])
            })
        {
            return Err(unavailable(
                "carrier comparison exceeds its proved original coefficient domain",
            ));
        }
        if self.record.literal {
            return Ok(0.0);
        }
        let u = Interval::point(u.lo.abs().max(u.hi.abs()));
        let v = Interval::point(v.lo.abs().max(v.hi.abs()));
        let [c0, cu, cv, cuv] = self.record.error_coefficients.map(Interval::point);
        let error = c0.add(cu.mul(u)).add(cv.mul(v)).add(cuv.mul(u.mul(v)));
        Ok(Interval::new(error.lo, error.hi)?.hi)
    }
}
