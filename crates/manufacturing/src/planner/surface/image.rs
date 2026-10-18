// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;
use spiling_contracts::{
    geometry::{OccurrenceId, SourceHash},
    project::{ProjectError, ProjectErrorCode},
    surface::{ImageReliefParameters, SurfaceLayerId},
};
use spiling_surface_fields::{FieldBounds, HeightImage, ImageField};
use std::sync::Arc;

pub(super) struct ImageMapping {
    pub image: Arc<HeightImage>,
    pub parameters: ImageReliefParameters,
    pub layer_id: SurfaceLayerId,
    pub occurrence_id: OccurrenceId,
    pub min_z: f64,
    pub translation_z: f64,
    pub sigma: f64,
}
pub(super) fn map_error(e: ProjectError) -> ManufacturingError {
    error(
        match e.code {
            ProjectErrorCode::Cancelled => ManufacturingErrorCode::Cancelled,
            ProjectErrorCode::ResourceLimit => ManufacturingErrorCode::ResourceLimit,
            _ => ManufacturingErrorCode::InvalidSpecification,
        },
        e.to_string(),
    )
}
impl ImageMapping {
    pub fn new(
        input: &CompilerInput<'_>,
        occurrence: &OccurrenceRecord,
        hash: &SourceHash,
        parameters: &ImageReliefParameters,
        layer_id: SurfaceLayerId,
        min_z: f64,
        max_z: f64,
    ) -> Result<Self, ManufacturingError> {
        parameters.validate().map_err(map_error)?;
        let image = input.surface_images.get(hash).ok_or_else(|| {
            error(
                ManufacturingErrorCode::InvalidSpecification,
                "captured surface image is missing",
            )
        })?;
        let sigma = rotate(&occurrence.pose, [0.0, 0.0, 1.0])[2].signum();
        let result = Self {
            image: Arc::clone(image),
            parameters: parameters.clone(),
            layer_id,
            occurrence_id: occurrence.occurrence_id,
            min_z,
            translation_z: occurrence.pose.translation_mm[2],
            sigma,
        };
        result.field()?;
        let scale = occurrence
            .pose
            .translation_mm
            .into_iter()
            .chain([
                min_z,
                max_z,
                parameters.z_period_mm,
                parameters.z_phase_mm,
                parameters.depth_mm,
            ])
            .fold(1.0f64, |a, b| a.max(b.abs()));
        let dz = parameters.depth_mm * image.record().height as f64 / parameters.z_period_mm;
        let arithmetic = 8192.0 * f64::EPSILON * scale * (1.0 + dz);
        if !arithmetic.is_finite()
            || arithmetic > SURFACE_ROUNDING_MM / 8.0
            || scale / parameters.z_period_mm > (1u64 << 40) as f64
        {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "source Z/phase image evaluation exceeds fixed arithmetic reserve",
            ));
        }
        Ok(result)
    }
    pub fn field(&self) -> Result<ImageField<'_>, ManufacturingError> {
        let p = &self.parameters;
        self.image
            .field(
                p.u_repeats,
                p.u_phase_fraction,
                p.z_period_mm,
                p.z_phase_mm,
                self.min_z,
            )
            .map_err(map_error)
    }
    pub fn source_z(&self, world_z: f64) -> f64 {
        self.sigma * (world_z - self.translation_z)
    }
    pub fn value(&self, u: f64, world_z: f64) -> f64 {
        // Construction only receives structurally checked captured mappings.
        self.field()
            .expect("checked captured image mapping")
            .value(u, self.source_z(world_z))
            * self.parameters.depth_mm
    }
    pub fn global_du(&self) -> f64 {
        self.parameters.depth_mm
            * self.image.record().width as f64
            * self.parameters.u_repeats as f64
    }
    pub(super) fn occupied_interval_bounds(
        &self,
        world: [f64; 2],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<FieldBounds, ManufacturingError> {
        check_cancel(cancel)?;
        let lower = (world[0] - self.translation_z).next_down();
        let upper = (world[1] - self.translation_z).next_up();
        if !lower.is_finite() || !upper.is_finite() {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "image source-height envelope is nonfinite",
            ));
        }
        let source = if self.sigma > 0.0 {
            [lower, upper]
        } else {
            [-upper, -lower]
        };
        let record = self.image.record();
        let scan = (record.width as usize)
            .checked_mul(2 * record.height as usize + 3)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "image occupied-band scan overflow",
                )
            })?;
        super::super::boolean::charge(work, scan)?;
        Ok(self.field()?.interval_bounds(source[0], source[1]))
    }
    pub fn zero_row(&self, world_z: f64) -> bool {
        self.field()
            .expect("checked captured image mapping")
            .row_bounds(self.source_z(world_z))
            .maximum
            == 0.0
    }
}
impl PreparedFinish {
    pub(super) fn image(&self) -> Option<&ImageMapping> {
        match &self.shape {
            PreparedShape::Planar(f) => f.image.as_ref(),
            PreparedShape::Cylindrical(f) => f.image.as_ref(),
        }
    }
    /// Attribute only this captured image carrier's exact native island. Other
    /// islands of the same occurrence retain their full realized obligations.
    pub(in crate::planner) fn owns_native_island(
        &self,
        polygon: &geo::Polygon<f64>,
        sample_world_z: f64,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        check_cancel(cancel)?;
        if !sample_world_z.is_finite() {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "nonfinite native island sample",
            ));
        }
        let bounds = self.bounds();
        if self.image().is_none()
            || sample_world_z <= bounds.min[2]
            || sample_world_z >= bounds.max[2]
        {
            return Ok(false);
        }
        match &self.shape {
            PreparedShape::Planar(field) => {
                splice::owns_native_island(polygon, field, work, cancel)
            }
            PreparedShape::Cylindrical(field) => {
                cylindrical::owns_native_island(polygon, field, work, cancel)
            }
        }
    }
    pub(in crate::planner) fn allowed_vertical_error(&self) -> Option<f64> {
        self.image().map(|i| i.parameters.vertical_error_mm)
    }
    pub(in crate::planner) fn is_z_invariant(
        &self,
        start: f64,
        end: f64,
    ) -> Result<bool, ManufacturingError> {
        let Some(i) = self.image() else {
            return Ok(true);
        };
        if !start.is_finite() || !end.is_finite() {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "nonfinite image Z interval",
            ));
        }
        let bounds = self.bounds();
        let start = start.max(bounds.min[2]);
        let end = end.min(bounds.max[2]);
        if start >= end {
            return Ok(true);
        }
        Ok(i.field()?
            .is_z_invariant(i.source_z(start), i.source_z(end)))
    }
    pub(in crate::planner) fn vertical_error_bound(
        &self,
        start: f64,
        end: f64,
        sample: f64,
    ) -> Result<f64, ManufacturingError> {
        if !sample.is_finite() {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "nonfinite shared native/image sample height",
            ));
        }
        let Some(i) = self.image() else {
            return Ok(0.0);
        };
        if self.is_z_invariant(start, end)? {
            return Ok(0.0);
        }
        let bounds = self.bounds();
        let start = start.max(bounds.min[2]);
        let end = end.min(bounds.max[2]);
        if start >= end {
            return Ok(0.0);
        }
        let b = i
            .field()?
            .interval_bounds(i.source_z(start), i.source_z(end));
        let reserve = 4096.0
            * f64::EPSILON
            * (1.0 + start.abs() + end.abs() + i.translation_z.abs())
            * (1.0 + b.dz)
            * i.parameters.depth_mm;
        let sample_distance = (sample - start).abs().max((end - sample).abs());
        let bound =
            ((b.maximum - b.minimum).min(b.dz * sample_distance)) * i.parameters.depth_mm + reserve;
        if !bound.is_finite() {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                "image vertical error cannot be bounded",
            ));
        }
        Ok(bound)
    }
    pub(in crate::planner) fn append_z_events(
        &self,
        start: f64,
        end: f64,
        out: &mut Vec<f64>,
        max: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let Some(i) = self.image() else { return Ok(()) };
        let bounds = self.bounds();
        let low = start.max(bounds.min[2]);
        let high = end.min(bounds.max[2]);
        if low >= high {
            return Ok(());
        }
        let offset = out.len();
        i.field()?
            .append_z_events(i.source_z(low), i.source_z(high), out, max, cancel)
            .map_err(map_error)?;
        for z in &mut out[offset..] {
            *z = i.translation_z + i.sigma * *z;
        }
        Ok(())
    }
}
