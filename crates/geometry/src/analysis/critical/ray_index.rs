// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Conservative ray candidates only; the original edge functional owns proof.

use super::{GeometryError, GeometryErrorCode, GeometryWorkBudget, Interval};
use std::sync::atomic::AtomicBool;

struct Node {
    bounds: [Interval; 2],
    end: usize,
    edge: Option<usize>,
}

pub(in crate::analysis) struct RayEdgeIndex {
    nodes: Vec<Node>,
}

fn resource() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "sample ray index exceeds admitted capacity",
    )
}

impl RayEdgeIndex {
    pub(in crate::analysis) fn required_heap_bytes(edges: usize) -> Option<usize> {
        if edges == 0 {
            return Some(0);
        }
        edges
            .checked_mul(2)?
            .checked_sub(1)?
            .checked_mul(std::mem::size_of::<Node>())
    }

    pub(in crate::analysis) fn new(
        edges: usize,
        heap_limit: usize,
        mut edge_bounds: impl FnMut(usize) -> [Interval; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        let bytes = Self::required_heap_bytes(edges)
            .filter(|bytes| *bytes <= heap_limit)
            .ok_or_else(resource)?;
        let count = bytes / std::mem::size_of::<Node>();
        let mut index = Self { nodes: Vec::new() };
        index
            .nodes
            .try_reserve_exact(count)
            .map_err(|_| resource())?;
        if index.heap_bytes() > heap_limit {
            return Err(resource());
        }
        if edges != 0 {
            index.build(0, edges, &mut edge_bounds, work, cancel)?;
        }
        Ok(index)
    }

    fn build(
        &mut self,
        first: usize,
        count: usize,
        bounds: &mut impl FnMut(usize) -> [Interval; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<usize, GeometryError> {
        work.charge(1, cancel)?;
        let index = self.nodes.len();
        self.nodes.push(Node {
            bounds: [Interval::point(0.0); 2],
            end: index + 1,
            edge: None,
        });
        let node_bounds = if count == 1 {
            self.nodes[index].edge = Some(first);
            bounds(first)
        } else {
            let half = count / 2;
            let left = self.build(first, half, bounds, work, cancel)?;
            let right = self.build(first + half, count - half, bounds, work, cancel)?;
            std::array::from_fn(|axis| Interval {
                lo: self.nodes[left].bounds[axis]
                    .lo
                    .min(self.nodes[right].bounds[axis].lo),
                hi: self.nodes[left].bounds[axis]
                    .hi
                    .max(self.nodes[right].bounds[axis].hi),
            })
        };
        self.nodes[index].bounds = node_bounds.map(|value| {
            if finite(value) {
                value
            } else {
                Interval {
                    lo: f64::NEG_INFINITY,
                    hi: f64::INFINITY,
                }
            }
        });
        self.nodes[index].end = self.nodes.len();
        Ok(index)
    }

    pub(in crate::analysis) fn heap_bytes(&self) -> usize {
        self.nodes
            .capacity()
            .saturating_mul(std::mem::size_of::<Node>())
    }

    pub(in crate::analysis) fn visit(
        &self,
        direction: [Interval; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        mut visitor: impl FnMut(usize, &mut GeometryWorkBudget) -> Result<(), GeometryError>,
    ) -> Result<(), GeometryError> {
        let mut cursor = 0;
        while cursor < self.nodes.len() {
            work.charge(1, cancel)?;
            let node = &self.nodes[cursor];
            if ray_disjoint(node.bounds, direction) {
                cursor = node.end;
                continue;
            }
            if let Some(edge) = node.edge {
                visitor(edge, work)?;
            }
            cursor += 1;
        }
        Ok(())
    }
}

fn finite(value: Interval) -> bool {
    value.lo.is_finite() && value.hi.is_finite() && value.lo <= value.hi
}

/// A positive ray through any enclosed direction cannot meet this whole box.
/// Every division/product is rounded outward; overflow retains the candidates.
fn ray_disjoint(bounds: [Interval; 2], direction: [Interval; 2]) -> bool {
    if bounds
        .into_iter()
        .chain(direction)
        .any(|value| !finite(value))
    {
        return false;
    }
    for axis in 0..2 {
        let d = direction[axis];
        if d.lo <= 0.0 && d.hi >= 0.0 {
            continue;
        }
        let b = bounds[axis];
        let quotients = [b.lo / d.lo, b.lo / d.hi, b.hi / d.lo, b.hi / d.hi];
        if quotients.into_iter().any(|value| !value.is_finite()) {
            continue;
        }
        let low = quotients
            .into_iter()
            .fold(f64::INFINITY, f64::min)
            .next_down()
            .max(0.0);
        let high = quotients
            .into_iter()
            .fold(f64::NEG_INFINITY, f64::max)
            .next_up();
        if high < low {
            return true;
        }
        let other = 1 - axis;
        let d = direction[other];
        let products = [low * d.lo, low * d.hi, high * d.lo, high * d.hi];
        if products.into_iter().any(|value| !value.is_finite()) {
            continue;
        }
        let low = products
            .into_iter()
            .fold(f64::INFINITY, f64::min)
            .next_down();
        let high = products
            .into_iter()
            .fold(f64::NEG_INFINITY, f64::max)
            .next_up();
        if high < bounds[other].lo || low > bounds[other].hi {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    fn interval(lo: f64, hi: f64) -> Interval {
        Interval { lo, hi }
    }

    #[test]
    fn ray_exclusion_retains_closed_touches_axes_uncertainty_and_overflow() {
        let forward = [interval(2.0, 3.0), interval(-1.0, 1.0)];
        assert!(!ray_disjoint(
            forward,
            [Interval::point(1.0), Interval::point(0.0)]
        ));
        assert!(ray_disjoint(
            [interval(-3.0, -2.0), interval(-1.0, 1.0)],
            [Interval::point(1.0), Interval::point(0.0)]
        ));
        assert!(!ray_disjoint(
            [interval(0.0, 0.0), interval(2.0, 3.0)],
            [Interval::point(0.0), Interval::point(1.0)]
        ));
        assert!(!ray_disjoint(
            [interval(2.0, 3.0), interval(2.0, 3.0)],
            [Interval::point(1.0), Interval::point(1.0)]
        ));
        assert!(!ray_disjoint(
            forward,
            [interval(-1.0, 1.0), interval(-1.0, 1.0)]
        ));
        assert!(!ray_disjoint(
            forward,
            [Interval::point(f64::from_bits(1)), Interval::point(0.0)]
        ));
        assert!(ray_disjoint(
            [interval(2.0, 3.0), interval(4.0, 5.0)],
            [Interval::point(1.0), Interval::point(0.0)]
        ));
    }
}
