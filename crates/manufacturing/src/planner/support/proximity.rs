// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Conservative separation screen between a support column and one exclusion
//! operand. It only skips a subtraction whose clearance offset provably cannot
//! meet the column, so the skipped difference is the identity set operation.
//! It certifies nothing about final actual support capsules.

use geo::{MultiPolygon, Polygon};

/// Column boundary edges sorted by their lower X, with per-component bounds.
pub(super) struct ColumnProximity {
    edges: Vec<Edge>,
    widest: f64,
    components: Vec<Bounds>,
}

#[derive(Clone, Copy)]
struct Edge {
    a: [f64; 2],
    b: [f64; 2],
    min: [f64; 2],
    max: [f64; 2],
}

#[derive(Clone, Copy)]
struct Bounds {
    min: [f64; 2],
    max: [f64; 2],
    first: [f64; 2],
}

fn rings(polygon: &Polygon<f64>) -> impl Iterator<Item = &geo::LineString<f64>> {
    std::iter::once(polygon.exterior()).chain(polygon.interiors())
}

fn ring_edges(ring: &geo::LineString<f64>) -> impl Iterator<Item = ([f64; 2], [f64; 2])> + '_ {
    let n = ring.0.len();
    // Closed or open rings: always include the closing edge.
    (0..n).map(move |i| {
        let a = ring.0[i];
        let b = ring.0[(i + 1) % n];
        ([a.x, a.y], [b.x, b.y])
    })
}

impl ColumnProximity {
    pub(super) fn new(column: &MultiPolygon<f64>) -> Self {
        let mut edges = Vec::new();
        let mut components = Vec::with_capacity(column.0.len());
        let mut widest = 0.0f64;
        for polygon in &column.0 {
            let mut bounds = Bounds {
                min: [f64::INFINITY; 2],
                max: [f64::NEG_INFINITY; 2],
                first: polygon
                    .exterior()
                    .0
                    .first()
                    .map_or([f64::NAN; 2], |c| [c.x, c.y]),
            };
            for ring in rings(polygon) {
                for (a, b) in ring_edges(ring) {
                    let min = [a[0].min(b[0]), a[1].min(b[1])];
                    let max = [a[0].max(b[0]), a[1].max(b[1])];
                    widest = widest.max(max[0] - min[0]);
                    bounds.min = [bounds.min[0].min(min[0]), bounds.min[1].min(min[1])];
                    bounds.max = [bounds.max[0].max(max[0]), bounds.max[1].max(max[1])];
                    edges.push(Edge { a, b, min, max });
                }
            }
            components.push(bounds);
        }
        edges.sort_unstable_by(|p, q| p.min[0].total_cmp(&q.min[0]));
        Self {
            edges,
            widest,
            components,
        }
    }

    /// Upper bound of [`Self::bytes`] for an index of `column`.
    pub(super) fn bytes_for(column: &MultiPolygon<f64>) -> usize {
        let edges: usize = column
            .0
            .iter()
            .flat_map(rings)
            .map(|ring| ring.0.len())
            .sum();
        edges.saturating_mul(std::mem::size_of::<Edge>())
            + column.0.len().saturating_mul(std::mem::size_of::<Bounds>())
    }

    pub(super) fn bytes(&self) -> usize {
        self.edges.capacity() * std::mem::size_of::<Edge>()
            + self.components.capacity() * std::mem::size_of::<Bounds>()
    }

    /// True only when every point of `piece` is farther than `reach` from
    /// every point of the column (boundaries apart and neither encloses the
    /// other). Any non-finite or uncertain comparison answers false.
    pub(super) fn separated(&self, piece: &Polygon<f64>, reach: f64) -> bool {
        if !reach.is_finite() || reach < 0.0 {
            return false;
        }
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for ring in rings(piece) {
            for c in &ring.0 {
                if !c.x.is_finite() || !c.y.is_finite() {
                    return false;
                }
                lo = [lo[0].min(c.x), lo[1].min(c.y)];
                hi = [hi[0].max(c.x), hi[1].max(c.y)];
            }
        }
        if lo[0] > hi[0] {
            return false;
        }
        let lo = [lo[0] - reach, lo[1] - reach];
        let hi = [hi[0] + reach, hi[1] + reach];
        let near = |min: [f64; 2], max: [f64; 2]| {
            !(max[0] < lo[0] || min[0] > hi[0] || max[1] < lo[1] || min[1] > hi[1])
        };
        if !self.components.iter().any(|c| near(c.min, c.max)) {
            return true;
        }
        let reach2 = reach * reach;
        let start = self
            .edges
            .partition_point(|e| e.min[0] < lo[0] - self.widest);
        for edge in &self.edges[start..] {
            if edge.min[0] > hi[0] {
                break;
            }
            if !near(edge.min, edge.max) {
                continue;
            }
            for ring in rings(piece) {
                for (a, b) in ring_edges(ring) {
                    let emin = [a[0].min(b[0]) - reach, a[1].min(b[1]) - reach];
                    let emax = [a[0].max(b[0]) + reach, a[1].max(b[1]) + reach];
                    if edge.max[0] < emin[0]
                        || edge.min[0] > emax[0]
                        || edge.max[1] < emin[1]
                        || edge.min[1] > emax[1]
                    {
                        continue;
                    }
                    let d2 = segment_distance_squared(a, b, edge.a, edge.b);
                    if d2.partial_cmp(&reach2) != Some(std::cmp::Ordering::Greater) {
                        return false;
                    }
                }
            }
        }
        // Boundaries are farther apart than `reach`; only containment remains.
        let Some(vertex) = piece.exterior().0.first() else {
            return false;
        };
        if self.contains([vertex.x, vertex.y]) {
            return false;
        }
        for component in &self.components {
            if near(component.min, component.max) && polygon_contains(piece, component.first) {
                return false;
            }
        }
        true
    }

    fn contains(&self, p: [f64; 2]) -> bool {
        // Even-odd ray crossing over every column ring; holes subtract.
        let mut inside = false;
        for edge in &self.edges {
            if crosses(p, edge.a, edge.b) {
                inside = !inside;
            }
        }
        inside
    }
}

fn crosses(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> bool {
    (a[1] > p[1]) != (b[1] > p[1])
        && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
}

fn polygon_contains(polygon: &Polygon<f64>, p: [f64; 2]) -> bool {
    if !p[0].is_finite() || !p[1].is_finite() {
        return true;
    }
    let mut inside = false;
    for ring in rings(polygon) {
        for (a, b) in ring_edges(ring) {
            if crosses(p, a, b) {
                inside = !inside;
            }
        }
    }
    inside
}

fn point_segment_distance_squared(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + t * d[0] - p[0], a[1] + t * d[1] - p[1]];
    q[0] * q[0] + q[1] * q[1]
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn segment_distance_squared(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let (o1, o2, o3, o4) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
    if ((o1 > 0.0 && o2 < 0.0) || (o1 < 0.0 && o2 > 0.0))
        && ((o3 > 0.0 && o4 < 0.0) || (o3 < 0.0 && o4 > 0.0))
    {
        return 0.0;
    }
    point_segment_distance_squared(a, c, d)
        .min(point_segment_distance_squared(b, c, d))
        .min(point_segment_distance_squared(c, a, b))
        .min(point_segment_distance_squared(d, a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;

    fn square(x: f64, y: f64, s: f64) -> Polygon<f64> {
        polygon![(x: x, y: y), (x: x + s, y: y), (x: x + s, y: y + s), (x: x, y: y + s)]
    }

    #[test]
    fn separation_requires_distance_and_no_containment() {
        let column = MultiPolygon(vec![square(0.0, 0.0, 10.0)]);
        let index = ColumnProximity::new(&column);
        assert!(index.separated(&square(12.0, 0.0, 1.0), 1.5));
        assert!(!index.separated(&square(12.0, 0.0, 1.0), 2.5));
        // Piece inside the column, far from its boundary.
        assert!(!index.separated(&square(4.0, 4.0, 1.0), 0.5));
        // Column inside the piece.
        assert!(!index.separated(&square(-5.0, -5.0, 30.0), 0.5));
        // Piece inside a column hole, away from the hole boundary.
        let holed = MultiPolygon(vec![Polygon::new(
            square(0.0, 0.0, 10.0).exterior().clone(),
            vec![square(2.0, 2.0, 6.0).exterior().clone()],
        )]);
        let index = ColumnProximity::new(&holed);
        assert!(index.separated(&square(4.5, 4.5, 1.0), 1.0));
        assert!(!index.separated(&square(4.5, 4.5, 1.0), 3.0));
    }
}
