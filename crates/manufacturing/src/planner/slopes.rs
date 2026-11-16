// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Layer heights for stepped walls, decided once before planning.
//!
//! A wall line rests on the line below it when it steps out no more than
//! half a bead. Between two neighbouring layers, the part of the upper
//! section farther than that outside the lower one is wall that would hang.
//! A layer is halved when its midpoint section actually shrinks that excess
//! (a slope, which thinner layers anchor); a step whose excess stays the same
//! however thin the layers get (a ledge or ceiling) is left to support. The
//! schedule is settled here from sections alone, so the plan is constructed
//! once and never split and retried.

use super::{LayerElevation, native, region};
use crate::{check_cancel, error};
use geo::{Area, BooleanOps, MultiPolygon};
use spiling_contracts::manufacturing::*;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

/// The thinnest layer a step may ask for, as a fraction of the nominal
/// layer height. Steeper walls belong to support.
const STEP_FLOOR_FRACTION: f64 = 0.25;

/// Arithmetic roundoff of halving a millimetre-scale height, far below any
/// commanded Z step.
const FLOOR_ROUNDOFF_MM: f64 = 1e-9;

/// Split layers whose walls would hang past half a bead where thinner
/// layers anchor them. Only with support enabled, where the alternative is
/// scaffold; explicit Off prints authored overhangs as reported. Returns the
/// number of layers added.
pub(super) fn split_for_slopes(
    input: &super::CompilerInput<'_>,
    admission: &super::preflight::Admission,
    schedule: &mut Vec<LayerElevation>,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let intent = &input.intent;
    if matches!(intent.recipe.supports, SupportPolicy::Disabled) || schedule.len() < 2 {
        return Ok(0);
    }
    let width = intent.recipe.bead_width_mm;
    let nominal = intent.select_layer_height()?.height_mm;
    let floor = (nominal * STEP_FLOOR_FRACTION)
        .max(intent.effective_deposition_limits().min_bead_height_mm);
    let noise_mm2 = width * spiling_geometry::tolerant::TolerantPolicy::default().profile_mm;
    let mut sections = Sections {
        models: admission.models.values().map(|model| &**model).collect(),
        cached: BTreeMap::new(),
        points: 0,
    };
    let mut steps = Steps {
        width,
        noise_mm2,
        excess: BTreeMap::new(),
    };
    split_where_thinner_anchors(
        schedule,
        floor,
        |height| intent.admits_deposition(width, height),
        |lower, upper| steps.excess(&mut sections, lower, upper, cancel),
        noise_mm2,
        cancel,
    )
}

/// The samples of the halves `split_resolved_layer` makes.
fn half_samples(layer: &LayerElevation) -> [f64; 2] {
    let midpoint = layer.lower_z + (layer.z - layer.lower_z) * 0.5;
    [
        layer.lower_z + (midpoint - layer.lower_z) * 0.5,
        midpoint + (layer.z - midpoint) * 0.5,
    ]
}

/// Halve layers while it shrinks the hanging-wall excess between sampled
/// sections. `excess(lower_sample, upper_sample)` is that area; each pair
/// that cannot progress is settled once.
fn split_where_thinner_anchors(
    schedule: &mut Vec<LayerElevation>,
    floor: f64,
    admits: impl Fn(f64) -> bool,
    mut excess: impl FnMut(f64, f64) -> Result<f64, ManufacturingError>,
    noise_mm2: f64,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let before = schedule.len();
    let least = floor - FLOOR_ROUNDOFF_MM;
    let mut settled = BTreeSet::new();
    let mut index = 1;
    while index < schedule.len() {
        check_cancel(cancel)?;
        let pair = (
            schedule[index - 1].sample.to_bits(),
            schedule[index].sample.to_bits(),
        );
        if settled.contains(&pair) {
            index += 1;
            continue;
        }
        let hanging = excess(schedule[index - 1].sample, schedule[index].sample)?;
        // The thicker eligible layer of the pair; the first rests on the bed.
        let height = |layer: &LayerElevation| layer.z - layer.lower_z;
        let target = [index - 1, index]
            .into_iter()
            .filter(|&k| {
                let half = height(&schedule[k]) * 0.5;
                k >= 1 && half >= least && admits(half)
            })
            .max_by(|&a, &b| {
                height(&schedule[a])
                    .total_cmp(&height(&schedule[b]))
                    .then(a.cmp(&b))
            });
        let Some(target) = target.filter(|_| hanging > noise_mm2) else {
            settled.insert(pair);
            index += 1;
            continue;
        };
        // Excess over every pair the split replaces, before and after.
        let below = schedule[target - 1].sample;
        let above = schedule.get(target + 1).map(|layer| layer.sample);
        let middle = schedule[target].sample;
        let [lower, upper] = half_samples(&schedule[target]);
        let mut now = excess(below, middle)?;
        let mut split = excess(below, lower)? + excess(lower, upper)?;
        if let Some(above) = above {
            now += excess(middle, above)?;
            split += excess(upper, above)?;
        }
        if split < now - noise_mm2 {
            match native::split_resolved_layer(
                schedule,
                target as u32,
                least,
                "stepped wall layer height",
            ) {
                // Recheck from the lowest changed pair.
                Ok(()) => index = target.max(1),
                Err(failure) if failure.code == ManufacturingErrorCode::ResourceLimit => {
                    return Err(failure);
                }
                // A piece the process cannot command is left to support.
                Err(_) => {
                    settled.insert(pair);
                    index += 1;
                }
            }
        } else {
            settled.insert(pair);
            index += 1;
        }
    }
    Ok(schedule.len() - before)
}

/// Sections by sample height, cached while they fit the working set.
struct Sections<'a> {
    models: Vec<&'a spiling_geometry::tolerant::TolerantModel>,
    cached: BTreeMap<u64, MultiPolygon<f64>>,
    points: usize,
}

impl Sections<'_> {
    fn at(&mut self, z: f64, cancel: &AtomicBool) -> Result<MultiPolygon<f64>, ManufacturingError> {
        if let Some(section) = self.cached.get(&z.to_bits()) {
            return Ok(section.clone());
        }
        let mut section = MultiPolygon(Vec::new());
        for model in &self.models {
            check_cancel(cancel)?;
            let placed = super::tolerant::section_at(model, z)?;
            section = if section.0.is_empty() {
                placed
            } else {
                section.union(&placed)
            };
        }
        region::canonicalize(&mut section);
        let points = self.points.saturating_add(region::point_count(&section));
        let bytes = points.saturating_mul(std::mem::size_of::<geo::Coord<f64>>());
        // An optional cache: dropped when it does not fit, never refused.
        if super::remaining_geometry_units(points, bytes).is_ok() {
            self.points = points;
            self.cached.insert(z.to_bits(), section.clone());
        } else {
            self.cached.clear();
            self.points = 0;
        }
        Ok(section)
    }
}

/// Hanging-wall excess between two sampled sections, memoized per pair.
struct Steps {
    width: f64,
    noise_mm2: f64,
    excess: BTreeMap<(u64, u64), f64>,
}

impl Steps {
    fn excess(
        &mut self,
        sections: &mut Sections<'_>,
        lower: f64,
        upper: f64,
        cancel: &AtomicBool,
    ) -> Result<f64, ManufacturingError> {
        let key = (lower.to_bits(), upper.to_bits());
        if let Some(&area) = self.excess.get(&key) {
            return Ok(area);
        }
        let below = sections.at(lower, cancel)?;
        let above = sections.at(upper, cancel)?;
        let area = if above.0.is_empty() {
            0.0
        } else {
            let carried = region::offset_admitted(
                &below,
                self.width * 0.5,
                |_, bytes, overlay| {
                    if let Some((a, b)) = overlay {
                        super::boolean_budget(a, b, cancel)?;
                    }
                    if bytes > crate::working_set::bytes() {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "stepped-wall section offset exceeds the working set",
                        ));
                    }
                    Ok(())
                },
                cancel,
            )?;
            above.difference(&carried).unsigned_area()
        };
        let area = if area > self.noise_mm2 { area } else { 0.0 };
        self.excess.insert(key, area);
        Ok(area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule(layers: u32, height: f64) -> Vec<LayerElevation> {
        (0..layers)
            .map(|index| LayerElevation {
                lower_z: f64::from(index) * height,
                z: f64::from(index + 1) * height,
                sample: (f64::from(index) + 0.5) * height,
                height,
                course: index,
            })
            .collect()
    }

    /// A wall whose outline sits at `edge(z)` over a unit perimeter: the
    /// excess past half a bead between two samples.
    fn hanging(
        edge: impl Fn(f64) -> f64,
        width: f64,
    ) -> impl FnMut(f64, f64) -> Result<f64, ManufacturingError> {
        move |lower, upper| Ok((edge(upper) - edge(lower) - width * 0.5).max(0.0))
    }

    fn split(
        layers: &mut Vec<LayerElevation>,
        edge: impl Fn(f64) -> f64,
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        split_where_thinner_anchors(layers, 0.05, |_| true, hanging(edge, 0.45), 1e-6, cancel)
    }

    #[test]
    fn a_slope_is_thinned_until_its_walls_rest_on_the_layer_below() {
        let cancel = AtomicBool::new(false);
        // Steps out 1.5 mm per mm between z 0.8 and 1.2: 0.3 mm per 0.2 mm
        // layer, more than half a bead.
        let edge = |z: f64| 1.5 * (z.clamp(0.8, 1.2) - 0.8);
        let mut layers = schedule(10, 0.2);
        assert!(split(&mut layers, edge, &cancel).unwrap() > 0);
        for pair in layers.windows(2) {
            let step = edge(pair[1].sample) - edge(pair[0].sample);
            assert!(step <= 0.225 + 1e-9, "{step} at {}", pair[1].sample);
        }
        // Layers away from the slope keep their height; every boundary stays.
        assert!((layers[1].z - layers[1].lower_z - 0.2).abs() < 1e-9);
        for z in 1..=10 {
            assert!(
                layers
                    .iter()
                    .any(|layer| (layer.z - f64::from(z) * 0.2).abs() < 1e-9)
            );
        }
    }

    #[test]
    fn a_ledge_and_the_floor_are_left_to_support() {
        let cancel = AtomicBool::new(false);
        // A 5 mm ledge at z 1.05: thinner layers never shrink it.
        let mut layers = schedule(10, 0.2);
        let ledge = |z: f64| if z > 1.05 { 5.0 } else { 0.0 };
        assert_eq!(split(&mut layers, ledge, &cancel).unwrap(), 0);
        // A slope too steep for the floor stops at the floor.
        let mut layers = schedule(10, 0.2);
        let steep = |z: f64| 20.0 * (z.clamp(0.8, 1.2) - 0.8);
        split(&mut layers, steep, &cancel).unwrap();
        assert!(
            layers
                .iter()
                .all(|layer| layer.z - layer.lower_z >= 0.05 - 1e-9)
        );
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(split(&mut layers, steep, &cancel).is_err());
    }
}
