// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::resource;
use crate::check_cancel;
use geo::{BooleanOps, Coord, LineString, MultiPolygon, Polygon};
use i_overlay::{
    core::{fill_rule::FillRule, overlay_rule::OverlayRule},
    float::overlay::FloatOverlay,
    i_shape::source::resource::ShapeResource,
};
use spiling_contracts::manufacturing::ManufacturingError;
use std::{mem::size_of, sync::atomic::AtomicBool};

pub(super) fn intersection(
    subject: &MultiPolygon<f64>,
    clip: &MultiPolygon<f64>,
    available: usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    check_cancel(cancel)?;
    let subject_counts = counts(subject, cancel)?;
    let clip_counts = counts(clip, cancel)?;
    let required = bytes(subject_counts)?
        .checked_add(bytes(clip_counts)?)
        .ok_or_else(|| resource("wall intersection input capacity overflow"))?;
    if required > available {
        return Err(resource(
            "wall intersection input capacity exceeds byte budget",
        ));
    }
    check_cancel(cancel)?;
    let mut packed_subject = Paths::reserve(subject_counts)?;
    let mut packed_clip = Paths::reserve(clip_counts)?;
    if packed_subject.bytes().saturating_add(packed_clip.bytes()) > available {
        return Err(resource(
            "wall intersection input allocation exceeds byte budget",
        ));
    }
    packed_subject.load(subject, cancel)?;
    packed_clip.load(clip, cancel)?;
    check_cancel(cancel)?;
    let shapes = FloatOverlay::with_subj_and_clip(&packed_subject, &packed_clip)
        .overlay(OverlayRule::Intersect, FillRule::EvenOdd);
    check_cancel(cancel)?;
    let polygons = shapes
        .into_iter()
        .map(|shape| {
            let mut rings = shape.into_iter().map(|ring| {
                let mut result = LineString(ring.into_iter().map(Coord::from).collect());
                result.close();
                result
            });
            Polygon::new(rings.next().unwrap_or(LineString::empty()), rings.collect())
        })
        .collect();
    Ok(MultiPolygon(polygons))
}

struct Paths {
    coordinates: Vec<[f64; 2]>,
    ends: Vec<usize>,
}

impl Paths {
    fn bytes(&self) -> usize {
        (self.coordinates.capacity() * size_of::<[f64; 2]>())
            .saturating_add(self.ends.capacity() * size_of::<usize>())
    }

    fn reserve((points, rings): (usize, usize)) -> Result<Self, ManufacturingError> {
        let mut coordinates = Vec::new();
        coordinates
            .try_reserve_exact(points)
            .map_err(|_| resource("wall intersection coordinate allocation refused"))?;
        let mut ends = Vec::new();
        ends.try_reserve_exact(rings)
            .map_err(|_| resource("wall intersection ring allocation refused"))?;
        Ok(Self { coordinates, ends })
    }

    fn load(
        &mut self,
        source: &MultiPolygon<f64>,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        for ring in source.rings() {
            check_cancel(cancel)?;
            if ring.0.is_empty() {
                continue;
            }
            let points = &ring.0[..ring.0.len() - 1];
            for chunk in points.chunks(1024) {
                check_cancel(cancel)?;
                self.coordinates
                    .extend(chunk.iter().map(|point| [point.x, point.y]));
            }
            self.ends.push(self.coordinates.len());
        }
        Ok(())
    }
}

impl ShapeResource<[f64; 2], f64> for Paths {
    type ResourceIter<'a> = Rings<'a>;

    fn iter_paths(&self) -> Rings<'_> {
        Rings {
            coordinates: &self.coordinates,
            ends: self.ends.iter(),
            start: 0,
        }
    }
}

struct Rings<'a> {
    coordinates: &'a [[f64; 2]],
    ends: std::slice::Iter<'a, usize>,
    start: usize,
}

impl<'a> Iterator for Rings<'a> {
    type Item = &'a [[f64; 2]];

    fn next(&mut self) -> Option<Self::Item> {
        let end = *self.ends.next()?;
        let ring = &self.coordinates[self.start..end];
        self.start = end;
        Some(ring)
    }
}

fn counts(
    source: &MultiPolygon<f64>,
    cancel: &AtomicBool,
) -> Result<(usize, usize), ManufacturingError> {
    source
        .rings()
        .try_fold((0usize, 0usize), |(points, rings), ring| {
            check_cancel(cancel)?;
            points
                .checked_add(ring.0.len().saturating_sub(1))
                .zip(rings.checked_add(usize::from(!ring.0.is_empty())))
                .ok_or_else(|| resource("wall intersection input count overflow"))
        })
}

fn bytes((points, rings): (usize, usize)) -> Result<usize, ManufacturingError> {
    points
        .checked_mul(size_of::<[f64; 2]>())
        .zip(rings.checked_mul(size_of::<usize>()))
        .and_then(|(coordinates, ends)| coordinates.checked_add(ends))
        .ok_or_else(|| resource("wall intersection input byte overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;
    use spiling_contracts::manufacturing::ManufacturingErrorCode;

    fn circle(center: [f64; 2], radius: f64, points: usize) -> Polygon<f64> {
        Polygon::new(
            LineString(
                (0..points)
                    .map(|index| {
                        let angle = std::f64::consts::TAU * index as f64 / points as f64;
                        Coord {
                            x: center[0] + radius * angle.cos(),
                            y: center[1] + radius * angle.sin(),
                        }
                    })
                    .collect(),
            ),
            Vec::new(),
        )
    }

    #[test]
    fn packed_inputs_match_geo_across_frames_holes_and_sizes() {
        let cancel = AtomicBool::new(false);
        for origin in [0.0, 1_000_000.0, -1_000_000.0] {
            let mut annulus = circle([origin, origin], 2.0, 127);
            annulus.interiors_push(circle([origin, origin], 0.8, 41).exterior().clone());
            let source = MultiPolygon(vec![annulus, circle([origin + 5.0, origin], 1.0, 17)]);
            for index in 0..30 {
                let position = index as f64 * 0.21;
                let clip = MultiPolygon(vec![polygon![
                    (x: origin - 1.0 + position, y: origin - 3.0),
                    (x: origin + 0.1 + position, y: origin - 3.0),
                    (x: origin + 0.1 + position, y: origin + 3.0),
                    (x: origin - 1.0 + position, y: origin + 3.0),
                ]]);
                for (left, right) in [(&source, &clip), (&clip, &source), (&source, &source)] {
                    assert_eq!(
                        intersection(left, right, 1_000_000, &cancel).unwrap(),
                        left.intersection(right)
                    );
                }
            }
            for empty in [
                MultiPolygon(Vec::new()),
                MultiPolygon(vec![Polygon::empty()]),
            ] {
                for (left, right) in [(&empty, &source), (&source, &empty), (&empty, &empty)] {
                    assert_eq!(
                        intersection(left, right, 1_000_000, &cancel).unwrap(),
                        left.intersection(right)
                    );
                }
            }
        }
    }

    #[test]
    fn packed_inputs_keep_existing_scratch_bound_and_typed_limits() {
        let cancel = AtomicBool::new(false);
        let mut source = circle([0.0, 0.0], 2.0, 40);
        source.interiors_push(LineString::empty());
        let source = MultiPolygon(vec![source, Polygon::empty()]);
        let required = 2 * bytes(counts(&source, &cancel).unwrap()).unwrap();
        let existing_scratch = 2 * super::super::super::super::region::point_count(&source) * 48;
        assert!(required <= existing_scratch);
        assert_eq!(
            intersection(&source, &source, required - 1, &cancel)
                .unwrap_err()
                .code,
            ManufacturingErrorCode::ResourceLimit
        );
        assert_eq!(
            intersection(&source, &source, required, &cancel).unwrap(),
            source.intersection(&source)
        );
        assert_eq!(
            intersection(
                &MultiPolygon(vec![Polygon::empty()]),
                &MultiPolygon(Vec::new()),
                0,
                &cancel
            )
            .unwrap(),
            MultiPolygon(Vec::new())
        );
        assert_eq!(
            bytes((usize::MAX, 0)).unwrap_err().code,
            ManufacturingErrorCode::ResourceLimit
        );
    }

    #[test]
    fn cancelled_packing_never_publishes_geometry() {
        let cancel = AtomicBool::new(true);
        let source = MultiPolygon(vec![circle([0.0, 0.0], 2.0, 40)]);
        assert_eq!(
            intersection(&source, &source, 1_000_000, &cancel)
                .unwrap_err()
                .code,
            ManufacturingErrorCode::Cancelled
        );
        assert_eq!(
            intersection(
                &MultiPolygon(Vec::new()),
                &MultiPolygon(Vec::new()),
                0,
                &cancel
            )
            .unwrap_err()
            .code,
            ManufacturingErrorCode::Cancelled
        );
    }
}
