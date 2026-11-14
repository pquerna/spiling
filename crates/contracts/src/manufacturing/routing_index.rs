// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Indexed reservation lookup for per-path loops over a fixed-dual plan.
//!
//! The router answers exactly what [`FixedDualJob::assignment_for_path`] and
//! [`FixedDualJob::tool_for_path`] answer, including which refusal is
//! returned; it only replaces the scan over every reservation with a Z
//! prefix-maximum and an XY bounding-volume hierarchy. Every candidate it
//! returns is still decided by the same exact containment rule.

use super::routing::{
    admit_reservation, bead_xy_enclosure, outside_reservations, xy_contains, z_admits,
};
use super::*;

const LEAF: usize = 4;

/// A fixed-dual job borrowed with an index over its source reservations.
/// Build once per plan pass with [`FixedDualJob::router`].
pub struct FixedDualRouter<'a> {
    job: &'a FixedDualJob,
    /// `None` when a reservation has a non-finite bound: the router then
    /// scans exactly like the job (validation refuses such jobs anyway).
    index: Option<ReservationIndex>,
}

struct ReservationIndex {
    /// Reservation minimum Z, ascending.
    z_min: Vec<f64>,
    /// Maximum reservation maximum Z over `z_min[..=i]`.
    z_prefix_max: Vec<f64>,
    nodes: Vec<Node>,
    /// Assignment indices in leaf order.
    order: Vec<u32>,
}

struct Node {
    min: [f64; 2],
    max: [f64; 2],
    /// Leaf: `order[start..start + count]`. Interior (`count == 0`): children
    /// are this node + 1 and `start`.
    start: u32,
    count: u32,
}

impl FixedDualJob {
    /// Borrow this job with a reservation index for per-path lookups.
    pub fn router(&self) -> Result<FixedDualRouter<'_>, ManufacturingError> {
        let finite = self.assignments.iter().all(|a| {
            let b = &a.work_envelope_mm;
            b.min.iter().chain(&b.max).all(|v| v.is_finite())
        });
        let index = if finite && !self.assignments.is_empty() {
            Some(ReservationIndex::build(&self.assignments)?)
        } else {
            None
        };
        Ok(FixedDualRouter { job: self, index })
    }
}

impl std::ops::Deref for FixedDualRouter<'_> {
    type Target = FixedDualJob;

    fn deref(&self) -> &FixedDualJob {
        self.job
    }
}

impl<'a> FixedDualRouter<'a> {
    pub fn job(&self) -> &'a FixedDualJob {
        self.job
    }

    /// Identical to [`FixedDualJob::assignment_for_path`].
    pub fn assignment_for_path(&self, path: &DepositionPath) -> Result<usize, ManufacturingError> {
        let Some(index) = &self.index else {
            return self.job.assignment_for_path(path);
        };
        let [z0, z1] = path.physical_z_mm;
        if z0.is_nan() || z1.is_nan() {
            return self.job.assignment_for_path(path);
        }
        if !index.any_z(z0, z1) {
            return Err(outside_reservations());
        }
        let (lower, upper) = bead_xy_enclosure(path)?;
        if !lower.iter().all(|v| v.is_finite()) {
            return self.job.assignment_for_path(path);
        }
        let assignments = &self.job.assignments;
        let mut found = None;
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);
        while let Some(node) = stack.pop() {
            let node_index = node as usize;
            let node = &index.nodes[node_index];
            if !(0..2).all(|axis| node.min[axis] <= lower[axis] && lower[axis] <= node.max[axis]) {
                continue;
            }
            if node.count == 0 {
                stack.push(node_index as u32 + 1);
                stack.push(node.start);
                continue;
            }
            let leaf = &index.order[node.start as usize..(node.start + node.count) as usize];
            for &candidate in leaf {
                let bounds = &assignments[candidate as usize].work_envelope_mm;
                if z_admits(bounds, path) && xy_contains(bounds, lower, upper) {
                    admit_reservation(&mut found, candidate as usize)?;
                }
            }
        }
        found.ok_or_else(outside_reservations)
    }

    /// Identical to [`FixedDualJob::tool_for_path`].
    pub fn tool_for_path(&self, path: &DepositionPath) -> Result<u8, ManufacturingError> {
        self.job
            .tool_for_path_with(path, |path| self.assignment_for_path(path))
    }
}

impl ReservationIndex {
    fn build(assignments: &[OccurrenceMaterialAssignment]) -> Result<Self, ManufacturingError> {
        let n = assignments.len();
        let allocation = || resource("fixed-dual reservation index allocation failed");
        let mut by_z: Vec<(f64, f64)> = Vec::new();
        by_z.try_reserve_exact(n).map_err(|_| allocation())?;
        by_z.extend(
            assignments
                .iter()
                .map(|a| (a.work_envelope_mm.min[2], a.work_envelope_mm.max[2])),
        );
        by_z.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut z_min = Vec::new();
        let mut z_prefix_max = Vec::new();
        z_min.try_reserve_exact(n).map_err(|_| allocation())?;
        z_prefix_max
            .try_reserve_exact(n)
            .map_err(|_| allocation())?;
        let mut running = f64::NEG_INFINITY;
        for (min, max) in by_z {
            running = running.max(max);
            z_min.push(min);
            z_prefix_max.push(running);
        }
        let mut order: Vec<u32> = Vec::new();
        order.try_reserve_exact(n).map_err(|_| allocation())?;
        order.extend(0..n as u32);
        let mut nodes: Vec<Node> = Vec::new();
        // Every leaf holds at least one entry, so the tree has < 2n nodes.
        nodes.try_reserve_exact(2 * n).map_err(|_| allocation())?;
        let mut pending = vec![(0usize, n, usize::MAX)];
        while let Some((start, end, parent)) = pending.pop() {
            let me = nodes.len();
            if parent != usize::MAX {
                // Second child: link it from the parent.
                nodes[parent].start = me as u32;
            }
            let entries = &mut order[start..end];
            let mut min = [f64::INFINITY; 2];
            let mut max = [f64::NEG_INFINITY; 2];
            let mut centre_min = [f64::INFINITY; 2];
            let mut centre_max = [f64::NEG_INFINITY; 2];
            for &entry in entries.iter() {
                let b = &assignments[entry as usize].work_envelope_mm;
                for axis in 0..2 {
                    min[axis] = min[axis].min(b.min[axis]);
                    max[axis] = max[axis].max(b.max[axis]);
                    let centre = centre(b, axis);
                    centre_min[axis] = centre_min[axis].min(centre);
                    centre_max[axis] = centre_max[axis].max(centre);
                }
            }
            if entries.len() <= LEAF {
                nodes.push(Node {
                    min,
                    max,
                    start: start as u32,
                    count: entries.len() as u32,
                });
                continue;
            }
            let axis = usize::from(centre_max[1] - centre_min[1] > centre_max[0] - centre_min[0]);
            let middle = entries.len() / 2;
            entries.select_nth_unstable_by(middle, |a, b| {
                centre(&assignments[*a as usize].work_envelope_mm, axis)
                    .total_cmp(&centre(&assignments[*b as usize].work_envelope_mm, axis))
            });
            nodes.push(Node {
                min,
                max,
                start: 0,
                count: 0,
            });
            // Depth-first: the first child is built immediately after this
            // node (me + 1); the second is linked when it is popped.
            pending.push((start + middle, end, me));
            pending.push((start, start + middle, usize::MAX));
        }
        Ok(Self {
            z_min,
            z_prefix_max,
            nodes,
            order,
        })
    }

    /// Whether any reservation admits finite `[z0, z1]`, exactly as
    /// `z_admits`: `min <= z0` and `max >= z1`.
    fn any_z(&self, z0: f64, z1: f64) -> bool {
        let count = self.z_min.partition_point(|&min| min <= z0);
        count > 0 && self.z_prefix_max[count - 1] >= z1
    }
}

fn centre(bounds: &AabbMm, axis: usize) -> f64 {
    bounds.min[axis] * 0.5 + bounds.max[axis] * 0.5
}
