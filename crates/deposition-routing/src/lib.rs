// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Deposition-preserving routing over declared geometry. No machine qualification.
#![forbid(unsafe_code)]

mod clearance;
mod planner;
mod search;
mod travel;

pub use planner::{plan, plan_input_order};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointMm(pub [f64; 3]);
impl PointMm {
    pub fn distance(self, other: Self) -> f64 {
        (self.0[0] - other.0[0])
            .hypot(self.0[1] - other.0[1])
            .hypot(self.0[2] - other.0[2])
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundsMm {
    pub min: [f64; 3],
    pub max: [f64; 3],
}
impl BoundsMm {
    pub fn contains(self, p: PointMm) -> bool {
        (0..3).all(|a| self.min[a] <= p.0[a] && p.0[a] <= self.max[a])
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrokeId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessKey {
    pub tool: u32,
    pub material: u32,
    /// Equal keys promise compatible deposition process/ownership; no inferred bond.
    pub continuity_group: u32,
}
#[derive(Clone, Debug)]
pub struct Stroke {
    pub id: StrokeId,
    pub points_mm: Vec<PointMm>,
    /// One value per actual edge, including the explicit closing edge.
    pub widths_mm: Vec<f64>,
    pub physical_z_mm: [f64; 2],
    pub process: ProcessKey,
    pub prerequisites: Vec<StrokeId>,
    pub allow_reverse: bool,
    pub allow_seam_rotation: bool,
}
#[derive(Clone, Debug)]
pub struct Problem {
    pub strokes: Vec<Stroke>,
    pub start_mm: PointMm,
    /// Tip-coordinate travel/deposition envelope, not a printer's complete model.
    pub envelope_mm: BoundsMm,
    /// Relative axis-aligned declared head components, including inactive tools.
    pub head_mm: Vec<BoundsMm>,
    pub obstacles_mm: Vec<BoundsMm>,
    /// Permitted intermediate tip locations; visibility edges are always checked.
    pub waypoints_mm: Vec<PointMm>,
    pub travel_speed_mm_s: f64,
    pub deposition_speed_mm_s: f64,
}
#[derive(Clone, Debug)]
pub struct Step {
    pub stroke: usize,
    pub reverse: bool,
    pub seam: usize,
    /// Includes actual previous position and entry. One point for zero travel.
    pub travel_mm: Vec<PointMm>,
    pub continuous: bool,
}
#[derive(Clone, Debug)]
pub struct Route {
    pub steps: Vec<Step>,
    pub work: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    /// Search found no complete route in the declared waypoint graph; not a global impossibility proof.
    NoRoute,
    Resource,
    Cancelled,
}
/// Caller-owned research/runtime allowances. Exhaustion is not infeasibility.
#[derive(Clone, Copy)]
pub struct Limits {
    pub work: u64,
    pub edges: usize,
    pub waypoints: usize,
}
struct Meter<'a> {
    used: u64,
    limit: u64,
    cancel: &'a AtomicBool,
}
impl Meter<'_> {
    fn charge(&mut self, n: u64) -> Result<(), Error> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        self.used = self
            .used
            .checked_add(n)
            .filter(|v| *v <= self.limit)
            .ok_or(Error::Resource)?;
        Ok(())
    }
}
/// Material boxes are conservative supersets of each nominal planar bead.
pub(crate) fn bead_boxes(stroke: &Stroke) -> impl Iterator<Item = BoundsMm> + '_ {
    stroke
        .points_mm
        .windows(2)
        .zip(&stroke.widths_mm)
        .map(|(p, w)| BoundsMm {
            min: [
                p[0].0[0].min(p[1].0[0]) - w / 2.,
                p[0].0[1].min(p[1].0[1]) - w / 2.,
                stroke.physical_z_mm[0],
            ],
            max: [
                p[0].0[0].max(p[1].0[0]) + w / 2.,
                p[0].0[1].max(p[1].0[1]) + w / 2.,
                stroke.physical_z_mm[1],
            ],
        })
}
/// Original edge indexes and ordered points; widths never detach from their edge.
pub(crate) fn traversal(
    stroke: &Stroke,
    reverse: bool,
    seam: usize,
) -> Vec<(PointMm, PointMm, f64)> {
    let n = stroke.widths_mm.len();
    (0..n)
        .map(|offset| {
            let i = if reverse {
                (seam + n - 1 - offset) % n
            } else {
                (seam + offset) % n
            };
            if reverse {
                (
                    stroke.points_mm[i + 1],
                    stroke.points_mm[i],
                    stroke.widths_mm[i],
                )
            } else {
                (
                    stroke.points_mm[i],
                    stroke.points_mm[i + 1],
                    stroke.widths_mm[i],
                )
            }
        })
        .collect()
}
