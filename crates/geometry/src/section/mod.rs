// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Direct intersections of the immutable native BREP. No Boolean clipping solid,
//! meshing seed, fitted STEP chart or tolerance-labelled root grants authority.
mod atlas;
mod continuation;
mod cuts;
mod direct;
mod roots;
#[cfg(test)]
mod tests;
mod types;
use crate::{Definition, GeometryError, GeometryWorkBudget, PlaneMm};
pub(crate) use direct::{canonicalize_section_in_place, certify_plane_functional_family};
use spiling_contracts::geometry::{
    GeometryErrorCode, SECTION_PLANE_TOLERANCE_MM, SECTION_SAMPLING_TOLERANCE_MM,
};
use std::{cell::Cell, sync::atomic::AtomicBool};
pub use types::{
    NativeSection, SectionChartEndpoint, SectionCurve, SectionEndpoint, SectionGraphChart,
    SectionLoopMm, SectionSpan,
};
pub(crate) use types::{NativeSectionCertificate, SectionAuthority};

pub const MAX_PREPARED_SECTION_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PREPARED_SECTION_SOURCE_BYTES: usize = MAX_PREPARED_SECTION_BYTES - 4 * 1024 * 1024;

struct NativeCorrespondenceReceipt {
    edgeuse: solidcore::topology::EdgeuseId,
    allowance_mm: Cell<f64>,
}
/// Preparation borrows the one immutable BREP; it creates no second source owner.
pub struct PreparedSectionSource<'a> {
    definition: &'a Definition,
    native_correspondence: Vec<NativeCorrespondenceReceipt>,
}
impl<'a> PreparedSectionSource<'a> {
    pub(crate) fn definition(&self) -> &'a Definition {
        self.definition
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(
            self.native_correspondence
                .capacity()
                .saturating_mul(std::mem::size_of::<NativeCorrespondenceReceipt>()),
        )
    }
    pub fn section_with_byte_limit(
        &self,
        plane: PlaneMm,
        cancel: &AtomicBool,
        byte_limit: u32,
        work: &mut GeometryWorkBudget,
    ) -> Result<NativeSection, GeometryError> {
        self.section_for_query(&SectionQuery::standard(plane), cancel, byte_limit, work)
    }
    pub(crate) fn section_for_query(
        &self,
        query: &SectionQuery,
        cancel: &AtomicBool,
        byte_limit: u32,
        work: &mut GeometryWorkBudget,
    ) -> Result<NativeSection, GeometryError> {
        direct::intersect(
            self.definition,
            Some(self),
            query,
            byte_limit as usize,
            byte_limit as usize,
            work,
            cancel,
        )
    }

    fn native_correspondence_allowance(
        &self,
        edgeuse: solidcore::topology::EdgeuseId,
    ) -> Result<&Cell<f64>, GeometryError> {
        self.native_correspondence
            .binary_search_by_key(&edgeuse, |receipt| receipt.edgeuse)
            .map(|index| &self.native_correspondence[index].allowance_mm)
            .map_err(|_| indeterminate("native correspondence lost its immutable coedge owner"))
    }
}
pub fn prepare_section_source<'a>(
    definition: &'a Definition,
    remaining_prepared_bytes: usize,
    cancel: &AtomicBool,
    work: &mut GeometryWorkBudget,
) -> Result<PreparedSectionSource<'a>, GeometryError> {
    work.charge(1, cancel)?;
    let count = definition.brep().edgeuses().len();
    let limit = remaining_prepared_bytes.min(MAX_PREPARED_SECTION_SOURCE_BYTES);
    let required = count
        .checked_mul(std::mem::size_of::<NativeCorrespondenceReceipt>())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PreparedSectionSource<'a>>()))
        .ok_or_else(|| resource_error("native section receipt capacity overflow"))?;
    if required > limit {
        return Err(resource_error(
            "native section preparation exceeds source allowance",
        ));
    }
    let mut native_correspondence = Vec::new();
    native_correspondence
        .try_reserve_exact(count)
        .map_err(|_| resource_error("native section correspondence receipt allocation failed"))?;
    if native_correspondence
        .capacity()
        .checked_mul(std::mem::size_of::<NativeCorrespondenceReceipt>())
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PreparedSectionSource<'a>>()))
        .is_none_or(|bytes| bytes > limit)
    {
        return Err(resource_error(
            "native section allocated receipt capacity exceeds allowance",
        ));
    }
    for (edgeuse, _) in definition.brep().edgeuses().iter() {
        work.charge(1, cancel)?;
        native_correspondence.push(NativeCorrespondenceReceipt {
            edgeuse,
            allowance_mm: Cell::new(f64::INFINITY),
        });
    }
    let sort_work = (count as u64)
        .checked_mul(u64::from(count.max(1).ilog2()) + 1)
        .and_then(|work| work.checked_mul(4))
        .ok_or_else(|| resource_error("native section receipt sort work overflow"))?;
    work.charge(sort_work, cancel)?;
    native_correspondence.sort_unstable_by_key(|receipt| receipt.edgeuse);
    work.charge(1, cancel)?;
    Ok(PreparedSectionSource {
        definition,
        native_correspondence,
    })
}
pub(crate) struct SectionQuery {
    pub(crate) plane: PlaneMm,
    pub(crate) maximum_chord_error_mm: f64,
    pub(crate) maximum_plane_error_mm: f64,
}
impl SectionQuery {
    fn standard(plane: PlaneMm) -> Self {
        Self {
            plane,
            maximum_chord_error_mm: SECTION_SAMPLING_TOLERANCE_MM,
            maximum_plane_error_mm: SECTION_PLANE_TOLERANCE_MM,
        }
    }
}
pub(crate) fn section(
    definition: &Definition,
    plane: PlaneMm,
    cancel: &AtomicBool,
    byte_limit: u32,
    work: &mut GeometryWorkBudget,
) -> Result<NativeSection, GeometryError> {
    direct::intersect(
        definition,
        None,
        &SectionQuery::standard(plane),
        MAX_PREPARED_SECTION_BYTES,
        byte_limit as usize,
        work,
        cancel,
    )
}
/// Borrowed query service for the single prepared solid/material owner.
/// `byte_limit` excludes all caller-owned coexisting source and output buffers.
pub(crate) fn section_for_query(
    definition: &Definition,
    query: &SectionQuery,
    cancel: &AtomicBool,
    byte_limit: u32,
    work: &mut GeometryWorkBudget,
) -> Result<NativeSection, GeometryError> {
    direct::intersect(
        definition,
        None,
        query,
        byte_limit as usize,
        byte_limit as usize,
        work,
        cancel,
    )
}
pub(super) fn indeterminate(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::AnalysisIndeterminate, message)
}
pub(super) fn resource_error(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}

/// Every retained capacity and fixed interval-evaluation scratch coexists with
/// the output. Admission precedes growth; source geometry remains borrowed.
struct Admission {
    limit: usize,
    used: usize,
}
impl Admission {
    fn new(limit: usize) -> Result<Self, GeometryError> {
        let used = crate::parametric::PreparedValidatedCurve::scratch_bytes()
            + crate::parametric::SURFACE_FIRST_ENCLOSURE_SCRATCH_BYTES
            + 256 * 1024;
        if used > limit {
            return Err(resource_error(
                "native section interval scratch exceeds query allowance",
            ));
        }
        Ok(Self { limit, used })
    }
    fn push<T>(&mut self, values: &mut Vec<T>, value: T) -> Result<(), GeometryError> {
        if values.len() == values.capacity() {
            let old = values.capacity();
            let target = old
                .max(4)
                .checked_mul(2)
                .ok_or_else(|| resource_error("native section capacity overflow"))?;
            let bytes = target
                .checked_sub(old)
                .and_then(|n| n.checked_mul(std::mem::size_of::<T>()))
                .ok_or_else(|| resource_error("native section capacity overflow"))?;
            if self.used.checked_add(bytes).is_none_or(|n| n > self.limit) {
                return Err(resource_error(
                    "native section live source/root/span/output allowance exhausted",
                ));
            }
            values
                .try_reserve_exact(target - old)
                .map_err(|_| resource_error("native section allocation failed"))?;
            let actual = (values.capacity() - old) * std::mem::size_of::<T>();
            self.used = self
                .used
                .checked_add(actual)
                .ok_or_else(|| resource_error("native section capacity overflow"))?;
            if self.used > self.limit {
                return Err(resource_error(
                    "native section allocated capacity exceeds allowance",
                ));
            }
        }
        values.push(value);
        Ok(())
    }
}
