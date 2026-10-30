// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounded nominal cross sections over independently decoded straight strokes.
//! Flat caps keep breaks explicit. Overlaps are not a deposited-volume estimate.
use super::*;

pub const MAX_BEAD_INSTANCES: usize = 100_000;
const MAX_VERTEX_INVOCATIONS: usize = 40_000_000;
const CHORD_ERROR_MM: f64 = 0.001;
const PROJECTION_ERROR_MM: f64 = 0.001;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub start: [f32; 3],
    pub width: f32,
    pub end: [f32; 3],
    pub height: f32,
    pub normal: [f32; 2],
    pub color: [f32; 3],
    pub elliptical: u32,
    pub facets: u32,
    pub padding: f32,
}
#[derive(Debug)]
pub struct BeadBatch {
    pub kind: u8,
    pub tool: u32,
    pub instances: Vec<Instance>,
    pub events: Vec<(u32, u16)>,
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
            for instance in &batch.instances[range.start as usize..range.end as usize] {
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
    /// `reserved_peak` includes the raw input and active/staged centerline display.
    pub(super) fn project(
        bytes: &[u8],
        origin: DVec3,
        reserved_peak: usize,
    ) -> Result<Self, String> {
        let records = bytes[REPLAY_HEADER_BYTES..]
            .as_chunks::<REPLAY_RECORD_BYTES>()
            .0;
        let number =
            |r: &[u8], offset| f64::from_le_bytes(r[offset..offset + 8].try_into().unwrap());
        let position = |r: &[u8]| DVec3::new(number(r, 0), number(r, 8), number(r, 16)) - origin;
        let mut counts = BTreeMap::<(u8, u32), usize>::new();
        let mut count = 0usize;
        let mut radius = 0.0f64;
        for (i, record) in records.iter().enumerate() {
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
            let start = position(&records[i - 1]);
            let end = position(record);
            if (start.z - end.z).abs() > 0.000_000_500_001 {
                return Err("Nonhorizontal deposition needs a separate envelope model".into());
            }
            let delta = end - start;
            if delta.x.hypot(delta.y) <= f64::EPSILON {
                return Err("A deposited stroke has no resolvable XY direction".into());
            }
            // Include endpoint conversion, dimension conversion, normal conversion
            // and arithmetic at expanded vertices, rather than checking centers alone.
            for point in [start, end] {
                let p = point.as_vec3();
                let extent = point.abs().max_element() + width + height;
                let rounding = extent * f64::from(f32::EPSILON) * 8.0;
                let dimensions = (f64::from(width as f32) - width).abs()
                    + (f64::from(height as f32) - height).abs();
                if !p.is_finite()
                    || (p.as_dvec3() - point).length() + dimensions + rounding > PROJECTION_ERROR_MM
                {
                    return Err(
                        "Expanded beads exceed the 0.001 mm numerical display budget".into(),
                    );
                }
            }
            let tool = u32::from_le_bytes(record[76..80].try_into().unwrap());
            *counts.entry((record[46], tool)).or_default() += 1;
        }
        if count == 0 {
            return Err("No nominal deposition associations are available".into());
        }
        if count > MAX_BEAD_INSTANCES {
            return Err(format!(
                "Bead display limit is {MAX_BEAD_INSTANCES} strokes; this review has {count}"
            ));
        }
        let needed = (std::f64::consts::PI / (1.0 - (CHORD_ERROR_MM / radius).min(1.0)).acos())
            .ceil() as u32;
        let facets = needed.max(8).next_multiple_of(4);
        if facets > 128 || count.saturating_mul((facets as usize + 2) * 12) > MAX_VERTEX_INVOCATIONS
        {
            return Err("Bead display exceeds its bounded surface-work allowance".into());
        }
        let per_instance = 3 * std::mem::size_of::<Instance>() + std::mem::size_of::<(u32, u16)>();
        if reserved_peak
            .checked_add(16_384 + count * per_instance)
            .is_none_or(|n| n > spiling_contracts::geometry::MAX_DISPLAY_BUFFER_BYTES as usize)
        {
            return Err("Beads exceed the combined active/staged display budget".into());
        }
        let mut groups: BTreeMap<_, _> = counts
            .into_iter()
            .map(|((kind, tool), capacity)| {
                (
                    (kind, tool),
                    BeadBatch {
                        kind,
                        tool,
                        instances: Vec::with_capacity(capacity),
                        events: Vec::with_capacity(capacity),
                    },
                )
            })
            .collect();
        for (i, record) in records.iter().enumerate() {
            if record[47] & 2 == 0 {
                continue;
            }
            let start = position(&records[i - 1]);
            let end = position(record);
            let delta = end - start;
            let length = delta.x.hypot(delta.y);
            let tool = u32::from_le_bytes(record[76..80].try_into().unwrap());
            let kind = record[46];
            let batch = groups.get_mut(&(kind, tool)).unwrap();
            batch.instances.push(Instance {
                start: start.as_vec3().to_array(),
                end: end.as_vec3().to_array(),
                width: number(record, 48) as f32,
                height: (number(record, 64) - number(record, 56)) as f32,
                normal: [(-delta.y / length) as f32, (delta.x / length) as f32],
                color: COLORS[kind as usize].map(|n| f32::from(n) / 255.0),
                elliptical: u32::from(kind == 8),
                facets,
                padding: 0.0,
            });
            batch.events.push((
                i as u32,
                u16::from_le_bytes(record[44..46].try_into().unwrap()),
            ));
        }
        let batches: Vec<_> = groups.into_values().collect();
        let owned_bytes = 16_384
            + batches
                .iter()
                .map(|b| {
                    2 * b.instances.capacity() * std::mem::size_of::<Instance>()
                        + b.events.capacity() * std::mem::size_of::<(u32, u16)>()
                })
                .sum::<usize>();
        Ok(Self {
            batches,
            facets,
            instance_count: count,
            chord_error_mm: radius * (1.0 - (std::f64::consts::PI / f64::from(facets)).cos()),
            owned_bytes,
        })
    }
}
