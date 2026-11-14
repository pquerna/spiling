// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounded nominal cross sections over independently decoded straight strokes.
//! Flat caps keep breaks explicit. Overlaps are not a deposited-volume estimate.
use super::*;

const CHORD_ERROR_MM: f64 = 0.001;
const PROJECTION_ERROR_MM: f64 = 0.001;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub start: [f32; 3],
    pub width: f32,
    pub end: [f32; 3],
    pub height: f32,
    /// Low four bits are the role; remaining bits hold cross-section facets.
    pub class: u32,
}
#[derive(Debug)]
pub struct BeadBatch {
    pub kind: u8,
    pub tool: u32,
    pub instances: std::sync::Arc<super::table::Table>,
    pub events: std::sync::Arc<Vec<(u32, u32)>>,
}
impl BeadBatch {
    pub fn range(&self, view: View) -> Range<u32> {
        event_range(&self.events, self.kind, self.tool, view, 1)
    }
}
#[derive(Debug)]
pub struct Beads {
    pub batches: Vec<BeadBatch>,
    pub facets: u32,
    pub instance_count: usize,
    pub chord_error_mm: f64,
    pub owned_bytes: usize,
}
#[derive(Debug, Clone, Copy)]
pub struct Statistics {
    pub count: usize,
    pub width: [f32; 2],
    pub height: [f32; 2],
}
impl Beads {
    pub fn statistics(&self, view: View) -> Option<Statistics> {
        let mut result: Option<Statistics> = None;
        for batch in &self.batches {
            let range = batch.range(view);
            if range.is_empty() {
                continue;
            }
            if range.start == 0 && range.end as usize == batch.instances.len() {
                let next = batch.instances.statistics();
                if let Some(stats) = &mut result {
                    stats.count += next.count;
                    stats.width = [
                        stats.width[0].min(next.width[0]),
                        stats.width[1].max(next.width[1]),
                    ];
                    stats.height = [
                        stats.height[0].min(next.height[0]),
                        stats.height[1].max(next.height[1]),
                    ];
                } else {
                    result = Some(next);
                }
                continue;
            }
            let data = batch.instances.load().ok()?;
            for instance in &data[range.start as usize..range.end as usize] {
                if let Some(stats) = &mut result {
                    stats.count += 1;
                    stats.width[0] = stats.width[0].min(instance.width);
                    stats.width[1] = stats.width[1].max(instance.width);
                    stats.height[0] = stats.height[0].min(instance.height);
                    stats.height[1] = stats.height[1].max(instance.height);
                } else {
                    result = Some(Statistics {
                        count: 1,
                        width: [instance.width; 2],
                        height: [instance.height; 2],
                    });
                }
            }
        }
        result
    }
    pub fn vertices_per_instance(&self) -> u32 {
        (self.facets + 2) * 12
    }
    pub(super) fn project_records(
        records: &super::records::Records,
        origin: DVec3,
    ) -> Result<Self, String> {
        let number =
            |r: &[u8], offset| f64::from_le_bytes(r[offset..offset + 8].try_into().unwrap());
        let position = |r: &[u8]| DVec3::new(number(r, 0), number(r, 8), number(r, 16)) - origin;
        let mut count = 0usize;
        let mut radius = 0.0f64;
        for i in 0..records.count {
            let record = &records.get(i)?;
            if record[47] & 2 == 0 {
                continue;
            }
            if i == 0 {
                return Err("A deposited stroke has no decoded start".into());
            }
            count += 1;
            let width = number(record, 48);
            let height = number(record, 64) - number(record, 56);
            if !(width as f32).is_finite()
                || (width as f32) < 2.0 * f32::MIN_POSITIVE
                || !(height as f32).is_finite()
                || (height as f32) < 2.0 * f32::MIN_POSITIVE
            {
                return Err("Nominal dimensions cannot be represented by the bead display".into());
            }
            let bridge = record[46] == 8;
            if !bridge && width < height {
                return Err("A nominal width is narrower than its stadium height".into());
            }
            radius = radius.max(if bridge { width.max(height) } else { height } * 0.5);
            let start = position(&records.get(i - 1)?);
            let end = position(record);
            if (start.z - end.z).abs() > 0.000_000_500_001 {
                return Err("Nonhorizontal deposition needs a separate envelope model".into());
            }
            let delta = end - start;
            if delta.x.hypot(delta.y) <= f64::EPSILON {
                return Err("A deposited stroke has no resolvable XY direction".into());
            }
            let projected = end.as_vec3() - start.as_vec3();
            let projected_length = projected.x.hypot(projected.y);
            let actual_length = delta.x.hypot(delta.y);
            if projected_length <= f32::MIN_POSITIVE {
                return Err("Nominal bead direction is lost in numerical projection".into());
            }
            let normal = glam::DVec2::new(
                f64::from(-projected.y / projected_length),
                f64::from(projected.x / projected_length),
            );
            let normal_error = (normal
                - glam::DVec2::new(-delta.y / actual_length, delta.x / actual_length))
            .length()
                * width
                * 0.5;
            // Include endpoint conversion, dimension conversion, normal conversion
            // and arithmetic at expanded vertices, rather than checking centers alone.
            for point in [start, end] {
                let p = point.as_vec3();
                let extent = point.abs().max_element() + width + height;
                let rounding = extent * f64::from(f32::EPSILON) * 8.0;
                let dimensions = (f64::from(width as f32) - width).abs()
                    + (f64::from(height as f32) - height).abs();
                if !p.is_finite()
                    || (p.as_dvec3() - point).length() + dimensions + rounding + normal_error
                        > PROJECTION_ERROR_MM
                {
                    return Err(
                        "Expanded beads exceed the 0.001 mm numerical display budget".into(),
                    );
                }
            }
        }
        if count == 0 {
            return Err("No nominal deposition associations are available".into());
        }
        let needed = (std::f64::consts::PI / (1.0 - (CHORD_ERROR_MM / radius).min(1.0)).acos())
            .ceil() as u32;
        let facets = needed.max(8).next_multiple_of(4);
        if facets > 128 {
            return Err("Nominal bead section cannot meet its chord bound".into());
        }
        // Geometry is borrowed from the single compact line/bead table below.
        let batches: Vec<BeadBatch> = Vec::new();
        let owned_bytes = 0;
        Ok(Self {
            batches,
            facets,
            instance_count: count,
            chord_error_mm: radius * (1.0 - (std::f64::consts::PI / f64::from(facets)).cos()),
            owned_bytes,
        })
    }
}
