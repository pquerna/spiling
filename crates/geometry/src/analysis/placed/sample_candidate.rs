// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Derived world coordinates coexist with the unchanged native sample until a
//! complete sufficient proof wins. No BREP or source-face metadata is copied.

use super::{
    AtomicBool, GeometryError, GeometryErrorCode, GeometryWorkBudget, HeightFunctional, Interval,
    NativeSection, PlaneFrameMm, PlaneMm, SamplePlacement, certify_orientation, error, finite,
    forward, length, record_world_accuracy, sub, transformed_position, world_frame,
    world_plane_error,
};
use crate::SectionLoopMm;

pub(super) struct WorldSampleCandidate {
    pub(super) loops: Vec<SectionLoopMm>,
    pub(super) arithmetic: f64,
    plane_error: f64,
    retained_bytes: usize,
}

impl WorldSampleCandidate {
    pub(super) fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub(super) fn prepare(
        section: &NativeSection,
        placement: &SamplePlacement<'_>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let resource = || {
            error(
                GeometryErrorCode::ResourceLimit,
                "world sample candidate exceeds coexisting query capacity",
            )
        };
        let point_count = section
            .loops
            .iter()
            .try_fold(0usize, |count, ring| {
                count.checked_add(ring.points_mm.len())
            })
            .ok_or_else(resource)?;
        let required = point_count
            .checked_mul(std::mem::size_of::<[f64; 3]>())
            .and_then(|bytes| {
                section
                    .loops
                    .len()
                    .checked_mul(std::mem::size_of::<SectionLoopMm>())
                    .and_then(|headers| bytes.checked_add(headers))
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>()))
            .filter(|bytes| *bytes <= remaining_bytes)
            .ok_or_else(resource)?;
        work.charge(1, cancel)?;
        let mut result = Self {
            loops: Vec::new(),
            arithmetic: 0.0,
            plane_error: 0.0,
            retained_bytes: std::mem::size_of::<Self>(),
        };
        result
            .loops
            .try_reserve_exact(section.loops.len())
            .map_err(|_| resource())?;
        result.retained_bytes += result.loops.capacity() * std::mem::size_of::<SectionLoopMm>();
        if result.retained_bytes() > remaining_bytes {
            return Err(resource());
        }
        let map = placement.map;
        let pose = placement.pose;
        let sample = placement.sample;
        result.plane_error = world_plane_error(
            section,
            HeightFunctional {
                direction: map.matrix[2],
                offset: sub(
                    Interval::point(pose.translation_mm[2]),
                    Interval::point(sample),
                ),
            },
            placement.extent,
        )?;
        certify_orientation(map.matrix, section.frame, world_frame())?;
        for ring in &section.loops {
            let mut points = Vec::new();
            ring.points_mm
                .len()
                .checked_mul(std::mem::size_of::<[f64; 3]>())
                .and_then(|bytes| bytes.checked_add(result.retained_bytes()))
                .filter(|bytes| *bytes <= remaining_bytes)
                .ok_or_else(resource)?;
            points
                .try_reserve_exact(ring.points_mm.len())
                .map_err(|_| resource())?;
            result.retained_bytes = points
                .capacity()
                .checked_mul(std::mem::size_of::<[f64; 3]>())
                .and_then(|bytes| bytes.checked_add(result.retained_bytes()))
                .filter(|bytes| *bytes <= remaining_bytes)
                .ok_or_else(resource)?;
            for point in &ring.points_mm {
                work.charge(1, cancel)?;
                let enclosure = transformed_position(map.matrix, pose, point.map(Interval::point))?;
                let actual = forward(pose, *point);
                if actual.iter().any(|value| !value.is_finite()) {
                    return Err(error(
                        GeometryErrorCode::ConditioningFailure,
                        "world sample candidate contains nonfinite transformed coordinates",
                    ));
                }
                result.arithmetic = result.arithmetic.max(
                    length(std::array::from_fn(|i| {
                        sub(enclosure[i], Interval::point(actual[i]))
                    }))?
                    .hi,
                );
                result.plane_error = result.plane_error.max(
                    finite(sub(Interval::point(actual[2]), Interval::point(sample)))?.abs_upper(),
                );
                points.push(actual);
            }
            result.loops.push(SectionLoopMm {
                is_hole: ring.is_hole,
                points_mm: points,
            });
            debug_assert!(result.retained_bytes() <= remaining_bytes);
        }
        debug_assert!(result.retained_bytes() >= required);
        Ok(result)
    }

    pub(super) fn commit(
        self,
        section: &mut NativeSection,
        placement: &SamplePlacement<'_>,
        query_byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        record_world_accuracy(section, placement.sigma, self.plane_error, self.arithmetic)?;
        section.loops = self.loops;
        section.plane = PlaneMm {
            origin_mm: [0.0, 0.0, placement.sample],
            normal: [0.0, 0.0, 1.0],
        };
        section.frame = PlaneFrameMm {
            origin_mm: section.plane.origin_mm,
            ..world_frame()
        };
        crate::section::canonicalize_section_in_place(
            section,
            query_byte_limit as u32,
            work,
            cancel,
        )?;
        Ok(self.arithmetic)
    }
}
