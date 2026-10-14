// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Immutable canonical raster ownership and the frozen periodic pixel-node lattice.
use spiling_contracts::{
    project::{ProjectError, ProjectErrorCode},
    surface::SurfaceImageRecord,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

mod normalize;
pub use normalize::normalize;

fn invalid(message: &str) -> ProjectError {
    ProjectError {
        code: ProjectErrorCode::InvalidProject,
        message: message.into(),
    }
}
fn cancelled(cancel: &AtomicBool) -> Result<(), ProjectError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ProjectError {
            code: ProjectErrorCode::Cancelled,
            message: "image operation cancelled".into(),
        })
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub struct HeightImage {
    record: SurfaceImageRecord,
    bytes: Arc<Vec<u8>>,
    pixel_offset: usize,
}
impl HeightImage {
    pub fn from_canonical(
        record: SurfaceImageRecord,
        bytes: Arc<Vec<u8>>,
    ) -> Result<Self, ProjectError> {
        if bytes.capacity() > spiling_contracts::surface::MAX_SURFACE_IMAGE_BYTES {
            return Err(ProjectError {
                code: ProjectErrorCode::ResourceLimit,
                message: "canonical image allocation capacity exceeds limit".into(),
            });
        }
        let view = record.validate_content(bytes.as_slice())?;
        let pixel_offset = bytes.len() - view.pixels.len();
        Ok(Self {
            record,
            bytes,
            pixel_offset,
        })
    }
    pub fn record(&self) -> &SurfaceImageRecord {
        &self.record
    }
    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }
    pub fn pixels(&self) -> &[u8] {
        &self.bytes[self.pixel_offset..]
    }
    pub fn field(
        &self,
        repeats: u32,
        phase: f64,
        period: f64,
        z_phase: f64,
        min_z: f64,
    ) -> Result<ImageField<'_>, ProjectError> {
        if !(1..=256).contains(&repeats)
            || !phase.is_finite()
            || !(0.0..1.0).contains(&phase)
            || !period.is_finite()
            || period <= 0.0
            || !z_phase.is_finite()
            || !(0.0..period).contains(&z_phase)
            || !min_z.is_finite()
        {
            return Err(invalid("invalid periodic image mapping"));
        }
        Ok(ImageField {
            image: self,
            repeats,
            phase,
            period,
            z_phase,
            min_z,
        })
    }
}

/// Bounds in normalized source U and millimetres of source Z. Second U derivative
/// is zero strictly inside a lattice cell; discontinuities are explicit events.
#[derive(Debug, Clone, Copy)]
pub struct FieldBounds {
    pub minimum: f64,
    pub maximum: f64,
    pub du: f64,
    pub duu: f64,
    pub dz: f64,
}
#[derive(Clone, Copy)]
pub struct ImageField<'a> {
    image: &'a HeightImage,
    repeats: u32,
    phase: f64,
    period: f64,
    z_phase: f64,
    min_z: f64,
}
impl ImageField<'_> {
    fn row(&self, z: f64) -> (usize, usize, f64) {
        let y = -(self.image.record.height as f64)
            * ((z - self.min_z + self.z_phase) / self.period).rem_euclid(1.0);
        let j = y.floor();
        let h = self.image.record.height as i64;
        (
            (j as i64).rem_euclid(h) as usize,
            (j as i64 + 1).rem_euclid(h) as usize,
            y - j,
        )
    }
    fn node(&self, i: usize, j: usize) -> f64 {
        self.image.pixels()[j * self.image.record.width as usize + i] as f64 / 255.0
    }
    pub fn value(&self, u: f64, z: f64) -> f64 {
        let x =
            self.image.record.width as f64 * (self.repeats as f64 * u + self.phase).rem_euclid(1.0);
        let floor = x.floor();
        // Floating rem_euclid can round a tiny negative residue to exactly1.
        // Wrap the integer node as well; clamping selects the wrong seam cell.
        let i = floor as usize % self.image.record.width as usize;
        let a = x - floor;
        let next = (i + 1) % self.image.record.width as usize;
        let (j, k, b) = self.row(z);
        let low = self.node(i, j) * (1.0 - a) + self.node(next, j) * a;
        let high = self.node(i, k) * (1.0 - a) + self.node(next, k) * a;
        low * (1.0 - b) + high * b
    }
    pub fn cell(&self, u: f64) -> (u32, u32) {
        let q = self.repeats as f64 * u + self.phase;
        (
            (q.rem_euclid(1.0) * self.image.record.width as f64).floor() as u32
                % self.image.record.width,
            ((self.repeats as f64 * u).floor().max(0.0) as u32).min(self.repeats - 1),
        )
    }
    pub fn row_bounds(&self, z: f64) -> FieldBounds {
        let (j, k, b) = self.row(z);
        let w = self.image.record.width as usize;
        let mut bounds = FieldBounds {
            minimum: 1.0,
            maximum: 0.0,
            du: 0.0,
            duu: 0.0,
            dz: 0.0,
        };
        for i in 0..w {
            let a = self.node(i, j) * (1.0 - b) + self.node(i, k) * b;
            let n = (i + 1) % w;
            let next = self.node(n, j) * (1.0 - b) + self.node(n, k) * b;
            bounds.minimum = bounds.minimum.min(a);
            bounds.maximum = bounds.maximum.max(a);
            bounds.du = bounds
                .du
                .max((next - a).abs() * w as f64 * self.repeats as f64);
            bounds.dz = bounds.dz.max(
                (self.node(i, j) - self.node(i, k)).abs() * self.image.record.height as f64
                    / self.period,
            );
        }
        bounds
    }
    /// Exact extrema over U and a source-Z interval occur on pixel nodes/row
    /// boundaries. Iterate at most H rows, even across arbitrarily many periods.
    pub fn interval_bounds(&self, start: f64, end: f64) -> FieldBounds {
        let low = start.min(end);
        let high = start.max(end).min(low + self.period);
        let mut out = self.row_bounds(low);
        out.dz = 0.0;
        let step = self.period / self.image.record.height as f64;
        let first = ((low - self.min_z + self.z_phase) / step).floor() + 1.0;
        let mut previous = low;
        for n in 0..=self.image.record.height {
            let next = (self.min_z - self.z_phase + (first + n as f64) * step).min(high);
            if next <= previous {
                continue;
            }
            let interior = self.row_bounds(previous + (next - previous) / 2.0);
            let endpoint = self.row_bounds(next);
            out.minimum = out.minimum.min(endpoint.minimum);
            out.maximum = out.maximum.max(endpoint.maximum);
            out.du = out.du.max(endpoint.du);
            out.dz = out.dz.max(interior.dz);
            previous = next;
            if next == high {
                break;
            }
        }
        out
    }
    pub fn is_z_invariant(&self, start: f64, end: f64) -> bool {
        start == end || self.interval_bounds(start, end).dz == 0.0
    }
    pub fn append_u_events(
        &self,
        start: f64,
        end: f64,
        out: &mut Vec<f64>,
        max: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ProjectError> {
        self.events(
            start,
            end,
            self.image.record.width as f64 * self.repeats as f64,
            self.phase * self.image.record.width as f64,
            out,
            max,
            cancel,
        )
    }
    pub fn append_z_events(
        &self,
        start: f64,
        end: f64,
        out: &mut Vec<f64>,
        max: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ProjectError> {
        cancelled(cancel)?;
        if self.is_z_invariant(start, end) {
            return Ok(());
        }
        let scale = self.image.record.height as f64 / self.period;
        let offset = out.len();
        self.events(
            start,
            end,
            scale,
            (self.z_phase - self.min_z) * scale,
            out,
            max,
            cancel,
        )?;
        let h = self.image.record.height as i64;
        let w = self.image.record.width as usize;
        let mut kept = offset;
        for index in offset..out.len() {
            cancelled(cancel)?;
            let event = out[index];
            let row = (-((event - self.min_z + self.z_phase) * scale).round() as i64).rem_euclid(h);
            let previous = (row + 1).rem_euclid(h) as usize;
            let next = (row - 1).rem_euclid(h) as usize;
            let row = row as usize;
            let pixels = self.image.pixels();
            let redundant = (0..w).all(|column| {
                pixels[row * w + column] == pixels[previous * w + column]
                    && pixels[row * w + column] == pixels[next * w + column]
            });
            if !redundant {
                out[kept] = event;
                kept += 1;
            }
        }
        out.truncate(kept);
        Ok(())
    }
    fn events(
        &self,
        start: f64,
        end: f64,
        scale: f64,
        phase: f64,
        out: &mut Vec<f64>,
        max: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ProjectError> {
        cancelled(cancel)?;
        if !start.is_finite() || !end.is_finite() || !scale.is_finite() || !phase.is_finite() {
            return Err(invalid("nonfinite image event interval"));
        }
        let low = start.min(end);
        let high = start.max(end);
        let estimate = ((high - low) * scale).ceil() + 2.0;
        if !estimate.is_finite() || estimate > max.saturating_sub(out.len()) as f64 {
            return Err(ProjectError {
                code: ProjectErrorCode::ResourceLimit,
                message: "image lattice event budget exceeded".into(),
            });
        }
        let first = (low * scale + phase).ceil();
        let last = (high * scale + phase).floor();
        if first.abs().max(last.abs()) > (1u64 << 50) as f64 {
            return Err(invalid("image lattice event conditioning exceeded"));
        }
        for n in 0..=((last - first).max(-1.0) as i64) {
            cancelled(cancel)?;
            let event = (first + n as f64 - phase) / scale;
            if event > low && event < high {
                out.push(event);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::geometry::SourceHash;
    #[test]
    fn shared_cross_language_canonical_golden() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/protocol/surface-image.json"
        ))
        .unwrap();
        let record: SurfaceImageRecord = serde_json::from_value(fixture["record"].clone()).unwrap();
        let hex = fixture["data"].as_str().unwrap();
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|n| u8::from_str_radix(&hex[n..n + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let image = HeightImage::from_canonical(record, Arc::new(bytes)).unwrap();
        assert_eq!(image.pixels(), &[0, 1, 255, 128, 64, 17]);
        let (normalized, bytes) = normalize(
            image.bytes(),
            image.record().source_name.clone(),
            image.bytes().len(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(&normalized, image.record());
        assert_eq!(&bytes, image.bytes().as_ref());
    }
    fn image() -> HeightImage {
        let mut bytes = b"P5\n3 2\n255\n".to_vec();
        bytes.extend([0, 1, 255, 128, 64, 17]);
        let record = SurfaceImageRecord {
            image_hash: SourceHash::from_bytes(&bytes),
            width: 3,
            height: 2,
            byte_count: bytes.len() as u64,
            source_hash: SourceHash::from_bytes(&bytes),
            source_name: "golden.pgm".into(),
        };
        assert_eq!(
            record.image_hash.as_str(),
            "3e3fa3680ccd7a4a3e93904f48f14661d07486d5c835cf2ffffb9e9427a6051c"
        );
        HeightImage::from_canonical(record, Arc::new(bytes)).unwrap()
    }
    #[test]
    fn negative_roundoff_at_periodic_seam_wraps_both_sample_and_cell() {
        let image = image();
        let field = image.field(1, 0.0, 4.0, 0.0, 0.0).unwrap();
        let before_seam = -f64::EPSILON / 8.0;
        assert_eq!(field.value(before_seam, 2.0), 128.0 / 255.0);
        assert_eq!(field.cell(before_seam).0, 0);
    }
    #[test]
    fn asymmetric_node_phase_reverse_rows_and_wrap() {
        let image = image();
        let f = image.field(1, 0.0, 4.0, 0.0, 0.0).unwrap();
        assert_eq!(f.value(0.0, 0.0), 0.0);
        assert_eq!(f.value(2.0 / 3.0, 0.0), 1.0);
        assert_eq!(f.value(0.0, 2.0), 128.0 / 255.0);
        assert!((f.value(5.0 / 6.0, 0.0) - 0.5).abs() < 1e-14);
        assert_eq!(f.value(0.0, 4.0), f.value(1.0, 0.0));
        let shifted = image.field(1, 1.0 / 3.0, 4.0, 0.0, 0.0).unwrap();
        assert_eq!(shifted.value(0.0, 0.0), 1.0 / 255.0);
        assert!(!f.is_z_invariant(0.0, 4.0));
        let mut events = vec![];
        f.append_z_events(0.0, 4.0, &mut events, 8, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(events, vec![2.0]);
    }
    #[test]
    fn canonical_allocation_is_shared() {
        let image = image();
        let bytes = Arc::clone(image.bytes());
        let again =
            HeightImage::from_canonical(image.record().clone(), Arc::clone(&bytes)).unwrap();
        assert!(Arc::ptr_eq(image.bytes(), again.bytes()));
    }
    #[test]
    fn flat_runs_skip_only_truly_redundant_rows_and_owning_capacity_is_bounded() {
        let mut bytes = b"P5\n2 4\n255\n".to_vec();
        bytes.extend([0, 0, 255, 17, 0, 0, 0, 0]);
        let record = SurfaceImageRecord {
            image_hash: SourceHash::from_bytes(&bytes),
            source_hash: SourceHash::from_bytes(&bytes),
            width: 2,
            height: 4,
            byte_count: bytes.len() as u64,
            source_name: "flat-run.pgm".into(),
        };
        let image = HeightImage::from_canonical(record.clone(), Arc::new(bytes.clone())).unwrap();
        let field = image.field(1, 0.0, 4.0, 0.0, 0.0).unwrap();
        assert!(field.is_z_invariant(0.0, 2.0));
        assert!(!field.is_z_invariant(2.0, 3.0));
        let mut events = vec![-7.0];
        field
            .append_z_events(0.0, 4.0, &mut events, 16, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(events, vec![-7.0, 2.0, 3.0]);
        bytes.reserve(spiling_contracts::surface::MAX_SURFACE_IMAGE_BYTES + 1);
        assert!(HeightImage::from_canonical(record, Arc::new(bytes)).is_err());
    }
    #[test]
    fn interval_bounds_enclose_asymmetric_values_and_derivatives() {
        let image = image();
        let f = image.field(3, 0.17, 7.0, 0.31, -2.0).unwrap();
        let bounds = f.interval_bounds(-1.7, 2.9);
        for zi in 0..40 {
            let z = -1.7 + 4.6 * zi as f64 / 39.0;
            for ui in 0..120 {
                let u = ui as f64 / 120.0;
                let value = f.value(u, z);
                assert!(value >= bounds.minimum - 1e-14 && value <= bounds.maximum + 1e-14);
                assert!((f.value(u + 1e-7, z) - value).abs() <= bounds.du * 1e-7 + 1e-14);
                assert!((f.value(u, z + 1e-7) - value).abs() <= bounds.dz * 1e-7 + 1e-14);
            }
        }
        let mut events = Vec::new();
        assert!(
            f.append_u_events(0.0, 1.0, &mut events, 2, &AtomicBool::new(false))
                .is_err()
        );
        assert!(events.is_empty());
    }
}
