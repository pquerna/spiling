// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Exact clearance field in a narrow band around one layer's support
//! boundary.
//!
//! The support grid answers set questions conservatively, so its boundary
//! sits up to a few cells inside the exact answer. Near that boundary this
//! field gives the exact signed clearance
//! `F(p) = min over obstacles (dist(p, obstacle) − radius)` to the same exact
//! exclusions the sweep subtracted on the layer (occupancy, voids, nominal
//! clearance), each a source segment with its own clearance radius. `F` is
//! 1-Lipschitz, so a step of `F(p) − level` from `p` toward its nearest
//! obstacle point never crosses the level set: every point it passes keeps
//! `F ≥ level`. Walls and row ends placed with it therefore clear by
//! construction; the grid containment assertion and its exact proof still
//! check every emitted bead.
//!
//! Only source segments within reach of the requested query points are
//! kept, in a uniform bucket index over those points. Queries are flat batch
//! arithmetic over the bucket's segments (a CubeCL kernel can take them
//! over); a query outside the indexed band answers `−∞`, which every caller
//! treats as "do not move".

use super::grid::Raster;
use geo::Polygon;
use spiling_contracts::manufacturing::ManufacturingError;

/// Bucket edge in millimetres: about the reach of one clearance query, so a
/// short source segment lands in a few buckets and a query scans one.
const BUCKET_MM: f64 = 0.5;

/// One exact exclusion: the closed capsule of segment `ab` and `radius`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Obstacle {
    pub(super) a: [f64; 2],
    pub(super) b: [f64; 2],
    pub(super) radius: f64,
}

/// The nearest obstacle to a query point: its clearance `F` and the closest
/// point on its source segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Nearest {
    pub(super) clearance: f64,
    pub(super) closest: Option<[f64; 2]>,
}

/// Builds a field over the buckets holding the query points it was opened
/// with. Obstacles beyond `reach` of every query are dropped.
pub(super) struct FieldBuilder<'r, 'a> {
    raster: &'r Raster<'a>,
    origin: [f64; 2],
    cols: usize,
    rows: usize,
    /// Per bucket, whether a query can land in it.
    interest: Vec<bool>,
    reach: f64,
    obstacles: Vec<Obstacle>,
}

/// The exact clearance field: the kept obstacles and, per bucket, the ones
/// whose capsule widened by `reach` meets it.
pub(super) struct Field {
    origin: [f64; 2],
    cols: usize,
    rows: usize,
    reach: f64,
    /// Per bucket, whether it was indexed: only those hold every obstacle
    /// within reach.
    interest: Vec<bool>,
    obstacles: Vec<Obstacle>,
    /// Bucket `k`'s obstacles are `members[starts[k]..starts[k + 1]]`.
    starts: Vec<u32>,
    members: Vec<u32>,
}

impl<'r, 'a> FieldBuilder<'r, 'a> {
    /// A builder whose queries land within `spread` of `points` and need the
    /// exact clearance up to `reach`; `None` without points.
    pub(super) fn new(
        raster: &'r Raster<'a>,
        points: &[[f64; 2]],
        spread: f64,
        reach: f64,
    ) -> Result<Option<Self>, ManufacturingError> {
        let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
        for p in points {
            for axis in 0..2 {
                bounds[0][axis] = bounds[0][axis].min(p[axis]);
                bounds[1][axis] = bounds[1][axis].max(p[axis]);
            }
        }
        if points.is_empty()
            || !bounds.iter().flatten().all(|v| v.is_finite())
            || !(spread.is_finite() && spread >= 0.0 && reach.is_finite() && reach > 0.0)
        {
            return Ok(None);
        }
        let origin = bounds[0].map(|v| v - spread - BUCKET_MM);
        let count = |axis: usize| {
            ((bounds[1][axis] + spread + BUCKET_MM - origin[axis]) / BUCKET_MM).ceil() as usize + 1
        };
        let (cols, rows) = (count(0), count(1));
        let mut interest = Vec::new();
        raster.reserve(&mut interest, cols.saturating_mul(rows))?;
        interest.resize(cols * rows, false);
        let mut builder = Self {
            raster,
            origin,
            cols,
            rows,
            interest,
            reach,
            obstacles: Vec::new(),
        };
        for p in points {
            if let Some([c0, c1, r0, r1]) = builder.range([*p, *p], spread) {
                for row in r0..=r1 {
                    builder.interest[row * cols + c0..=row * cols + c1].fill(true);
                }
            }
        }
        raster.charge(points.len());
        Ok(Some(builder))
    }

    /// Inclusive bucket columns and rows meeting `bounds` widened by `by`,
    /// clamped to the index.
    fn range(&self, bounds: [[f64; 2]; 2], by: f64) -> Option<[usize; 4]> {
        let lo =
            [0, 1].map(|axis| ((bounds[0][axis] - by - self.origin[axis]) / BUCKET_MM).floor());
        let hi =
            [0, 1].map(|axis| ((bounds[1][axis] + by - self.origin[axis]) / BUCKET_MM).floor());
        if !(lo.iter().chain(&hi).all(|v| v.is_finite()))
            || hi[0] < 0.0
            || hi[1] < 0.0
            || lo[0] >= self.cols as f64
            || lo[1] >= self.rows as f64
        {
            return None;
        }
        Some([
            lo[0].max(0.0) as usize,
            (hi[0] as usize).min(self.cols - 1),
            lo[1].max(0.0) as usize,
            (hi[1] as usize).min(self.rows - 1),
        ])
    }

    /// Whether an obstacle of `radius` inside `bounds` can come within reach
    /// of a query.
    pub(super) fn wants(&self, bounds: [[f64; 2]; 2], radius: f64) -> bool {
        let Some([c0, c1, r0, r1]) = self.range(bounds, radius + self.reach) else {
            return false;
        };
        self.raster.charge((c1 - c0 + 1) * (r1 - r0 + 1));
        (r0..=r1)
            .any(|row| self.interest[row * self.cols + c0..=row * self.cols + c1].contains(&true))
    }

    /// Adds the closed capsule `ab ⊕ radius` when it can reach a query.
    pub(super) fn segment(
        &mut self,
        a: [f64; 2],
        b: [f64; 2],
        radius: f64,
    ) -> Result<(), ManufacturingError> {
        if !(a.iter().chain(&b).all(|v| v.is_finite()) && radius.is_finite() && radius >= 0.0) {
            return Err(crate::error(
                spiling_contracts::manufacturing::ManufacturingErrorCode::NativeConditioningFailure,
                "support clearance field obstacle has non-finite coordinates",
            ));
        }
        let bounds = [
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        ];
        if !self.wants(bounds, radius) {
            return Ok(());
        }
        self.raster.reserve(&mut self.obstacles, 1)?;
        self.obstacles.push(Obstacle { a, b, radius });
        Ok(())
    }

    /// Adds every boundary edge of `polygon` at `radius`. Interiors are not
    /// represented: every query starts outside the sweep's exclusions.
    pub(super) fn polygon(
        &mut self,
        polygon: &Polygon<f64>,
        radius: f64,
    ) -> Result<(), ManufacturingError> {
        use geo::BoundingRect;
        let Some(rect) = polygon.bounding_rect() else {
            return Ok(());
        };
        if !self.wants(
            [[rect.min().x, rect.min().y], [rect.max().x, rect.max().y]],
            radius,
        ) {
            return Ok(());
        }
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            crate::check_cancel(self.raster.cancel)?;
            let n = ring.0.len();
            self.raster.charge(n);
            for i in 0..n {
                let (p, q) = (ring.0[i], ring.0[(i + 1) % n]);
                if p != q {
                    self.segment([p.x, p.y], [q.x, q.y], radius)?;
                }
            }
        }
        Ok(())
    }

    /// Indexes the kept obstacles by bucket.
    pub(super) fn finish(self) -> Result<Field, ManufacturingError> {
        let Self {
            raster,
            origin,
            cols,
            rows,
            interest,
            reach,
            obstacles,
        } = self;
        let buckets = cols * rows;
        let mut starts: Vec<u32> = Vec::new();
        raster.reserve(&mut starts, buckets + 1)?;
        starts.resize(buckets + 1, 0);
        let field = Field {
            origin,
            cols,
            rows,
            reach,
            interest: Vec::new(),
            obstacles: Vec::new(),
            starts: Vec::new(),
            members: Vec::new(),
        };
        let ranges = |o: &Obstacle| {
            field.range(
                [
                    [o.a[0].min(o.b[0]), o.a[1].min(o.b[1])],
                    [o.a[0].max(o.b[0]), o.a[1].max(o.b[1])],
                ],
                o.radius + reach,
            )
        };
        for o in &obstacles {
            if let Some([c0, c1, r0, r1]) = ranges(o) {
                for row in r0..=r1 {
                    for col in c0..=c1 {
                        if interest[row * cols + col] {
                            starts[row * cols + col + 1] += 1;
                        }
                    }
                }
                raster.charge((c1 - c0 + 1) * (r1 - r0 + 1));
            }
        }
        for k in 0..buckets {
            starts[k + 1] = starts[k].checked_add(starts[k + 1]).ok_or_else(|| {
                crate::error(
                    spiling_contracts::manufacturing::ManufacturingErrorCode::ResourceLimit,
                    "support clearance field index exceeds its integer range",
                )
            })?;
        }
        let total = starts[buckets] as usize;
        let mut members: Vec<u32> = Vec::new();
        raster.reserve(&mut members, total)?;
        members.resize(total, 0);
        let mut fill = starts.clone();
        for (index, o) in obstacles.iter().enumerate() {
            if let Some([c0, c1, r0, r1]) = ranges(o) {
                for row in r0..=r1 {
                    for col in c0..=c1 {
                        let k = row * cols + col;
                        if interest[k] {
                            members[fill[k] as usize] = index as u32;
                            fill[k] += 1;
                        }
                    }
                }
            }
        }
        drop(fill);
        Ok(Field {
            interest,
            obstacles,
            starts,
            members,
            ..field
        })
    }
}

impl Field {
    /// The least clearance radius of any kept obstacle (infinite without
    /// obstacles).
    pub(super) fn min_radius(&self) -> f64 {
        self.obstacles
            .iter()
            .map(|o| o.radius)
            .fold(f64::INFINITY, f64::min)
    }

    /// Bytes this field retains.
    pub(super) fn bytes(&self) -> usize {
        self.obstacles
            .capacity()
            .saturating_mul(std::mem::size_of::<Obstacle>())
            .saturating_add(self.interest.capacity())
            .saturating_add(self.starts.capacity().saturating_mul(4))
            .saturating_add(self.members.capacity().saturating_mul(4))
    }

    fn range(&self, bounds: [[f64; 2]; 2], by: f64) -> Option<[usize; 4]> {
        let lo =
            [0, 1].map(|axis| ((bounds[0][axis] - by - self.origin[axis]) / BUCKET_MM).floor());
        let hi =
            [0, 1].map(|axis| ((bounds[1][axis] + by - self.origin[axis]) / BUCKET_MM).floor());
        if !(lo.iter().chain(&hi).all(|v| v.is_finite()))
            || hi[0] < 0.0
            || hi[1] < 0.0
            || lo[0] >= self.cols as f64
            || lo[1] >= self.rows as f64
        {
            return None;
        }
        Some([
            lo[0].max(0.0) as usize,
            (hi[0] as usize).min(self.cols - 1),
            lo[1].max(0.0) as usize,
            (hi[1] as usize).min(self.rows - 1),
        ])
    }

    /// The obstacles of every bucket the closed box meets, or `None` when the
    /// box leaves the indexed band (an empty bucket outside the band has no
    /// complete obstacle list).
    fn buckets(&self, bounds: [[f64; 2]; 2]) -> Option<impl Iterator<Item = &Obstacle> + '_> {
        let inside = |axis: usize, v: f64| {
            let t = (v - self.origin[axis]) / BUCKET_MM;
            t.is_finite() && t >= 0.0 && t < if axis == 0 { self.cols } else { self.rows } as f64
        };
        if !(inside(0, bounds[0][0])
            && inside(0, bounds[1][0])
            && inside(1, bounds[0][1])
            && inside(1, bounds[1][1]))
        {
            return None;
        }
        let [c0, c1, r0, r1] = self.range(bounds, 0.0)?;
        if !(r0..=r1).all(|row| {
            self.interest[row * self.cols + c0..=row * self.cols + c1]
                .iter()
                .all(|&k| k)
        }) {
            return None;
        }
        Some((r0..=r1).flat_map(move |row| {
            (c0..=c1).flat_map(move |col| {
                let k = row * self.cols + col;
                self.members[self.starts[k] as usize..self.starts[k + 1] as usize]
                    .iter()
                    .map(|&m| &self.obstacles[m as usize])
            })
        }))
    }

    /// The exact clearance at `p`, capped at the field's reach (an answer of
    /// `reach` means "at least `reach`"); `−∞` outside the indexed band.
    /// `work` counts obstacle evaluations.
    pub(super) fn nearest(&self, p: [f64; 2], work: &mut usize) -> Nearest {
        let mut best = Nearest {
            clearance: self.reach,
            closest: None,
        };
        let Some(obstacles) = self.buckets([p, p]) else {
            return Nearest {
                clearance: f64::NEG_INFINITY,
                closest: None,
            };
        };
        for o in obstacles {
            *work += 1;
            let c = closest_on_segment(p, o.a, o.b);
            let f = (p[0] - c[0]).hypot(p[1] - c[1]) - o.radius;
            if f < best.clearance {
                best = Nearest {
                    clearance: f,
                    closest: Some(c),
                };
            }
        }
        best
    }

    /// Batch form of [`Self::nearest`] over flat points, in input order.
    pub(super) fn nearest_batch(
        &self,
        points: &[[f64; 2]],
        out: &mut Vec<Nearest>,
        work: &mut usize,
    ) {
        out.clear();
        out.extend(points.iter().map(|&p| self.nearest(p, work)));
    }

    /// The least clearance of any point of segment `ab`, capped at the
    /// field's reach; `−∞` when the segment leaves the indexed band.
    pub(super) fn segment_clearance(&self, a: [f64; 2], b: [f64; 2], work: &mut usize) -> f64 {
        let bounds = [
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        ];
        let Some(obstacles) = self.buckets(bounds) else {
            return f64::NEG_INFINITY;
        };
        let mut least = self.reach;
        for o in obstacles {
            *work += 1;
            least = least.min(segment_distance(a, b, o.a, o.b) - o.radius);
        }
        least
    }

    /// The first `t` in `[0, length]` at which `p + t·d` (unit `d`) comes
    /// within `level` clearance of an obstacle, or `None` when the whole
    /// segment keeps more, or leaves the indexed band.
    pub(super) fn first_hit(
        &self,
        p: [f64; 2],
        d: [f64; 2],
        length: f64,
        level: f64,
        work: &mut usize,
    ) -> Option<f64> {
        let q = [p[0] + d[0] * length, p[1] + d[1] * length];
        let bounds = [
            [p[0].min(q[0]), p[1].min(q[1])],
            [p[0].max(q[0]), p[1].max(q[1])],
        ];
        let obstacles = self.buckets(bounds)?;
        let mut first: Option<f64> = None;
        for o in obstacles {
            *work += 1;
            if let Some(t) = ray_capsule(p, d, o.a, o.b, o.radius + level)
                && t <= length
                && first.is_none_or(|f| t < f)
            {
                first = Some(t);
            }
        }
        first
    }
}

pub(super) fn closest_on_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let d = [b[0] - a[0], b[1] - a[1]];
    let squared = d[0] * d[0] + d[1] * d[1];
    if squared == 0.0 {
        return a;
    }
    let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / squared).clamp(0.0, 1.0);
    [a[0] + d[0] * t, a[1] + d[1] * t]
}

fn point_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let c = closest_on_segment(p, a, b);
    (p[0] - c[0]).hypot(p[1] - c[1])
}

fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Distance between closed segments `ab` and `cd`; zero when they meet.
pub(super) fn segment_distance(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let (d1, d2) = (cross(c, d, a), cross(c, d, b));
    let (d3, d4) = (cross(a, b, c), cross(a, b, d));
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return 0.0;
    }
    point_segment(a, c, d)
        .min(point_segment(b, c, d))
        .min(point_segment(c, a, b))
        .min(point_segment(d, a, b))
}

/// The least `t ≥ 0` with `p + t·d` (unit `d`) in the closed capsule
/// `ab ⊕ radius`, if any.
fn ray_capsule(p: [f64; 2], d: [f64; 2], a: [f64; 2], b: [f64; 2], radius: f64) -> Option<f64> {
    if point_segment(p, a, b) <= radius {
        return Some(0.0);
    }
    let mut first: Option<f64> = None;
    let mut take = |t: f64| {
        if t.is_finite() && t >= 0.0 && first.is_none_or(|f| t < f) {
            first = Some(t);
        }
    };
    // End disks.
    for c in [a, b] {
        let m = [p[0] - c[0], p[1] - c[1]];
        let half_b = m[0] * d[0] + m[1] * d[1];
        let disc = half_b * half_b - (m[0] * m[0] + m[1] * m[1] - radius * radius);
        if disc >= 0.0 {
            take(-half_b - disc.sqrt());
        }
    }
    // Side lines, within the segment's span.
    let e = [b[0] - a[0], b[1] - a[1]];
    let length = e[0].hypot(e[1]);
    if length > 0.0 {
        let e = [e[0] / length, e[1] / length];
        let n = [-e[1], e[0]];
        let offset = (p[0] - a[0]) * n[0] + (p[1] - a[1]) * n[1];
        let rate = d[0] * n[0] + d[1] * n[1];
        if rate != 0.0 {
            for side in [radius, -radius] {
                let t = (side - offset) / rate;
                let x = [p[0] + d[0] * t - a[0], p[1] + d[1] * t - a[1]];
                let u = x[0] * e[0] + x[1] * e[1];
                if (0.0..=length).contains(&u) {
                    take(t);
                }
            }
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_distance_meets_crossing_and_parallel_segments() {
        assert_eq!(
            segment_distance([0.0, 0.0], [2.0, 2.0], [0.0, 2.0], [2.0, 0.0]),
            0.0
        );
        assert!(
            (segment_distance([0.0, 0.0], [2.0, 0.0], [0.0, 1.0], [2.0, 1.0]) - 1.0).abs() < 1e-12
        );
        assert!(
            (segment_distance([0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]) - 1.0).abs() < 1e-12
        );
    }

    #[test]
    fn ray_capsule_enters_at_the_exact_offset() {
        // A point capsule is a circle: the ray along +x from the origin meets
        // radius 1 about (3, 0) at t = 2.
        let t = ray_capsule([0.0, 0.0], [1.0, 0.0], [3.0, 0.0], [3.0, 0.0], 1.0).unwrap();
        assert!((t - 2.0).abs() < 1e-12);
        // A horizontal segment at y = 2 widened by 0.5 is met at y = 1.5.
        let t = ray_capsule([0.0, 0.0], [0.0, 1.0], [-1.0, 2.0], [1.0, 2.0], 0.5).unwrap();
        assert!((t - 1.5).abs() < 1e-12);
        assert_eq!(
            ray_capsule([0.0, 0.0], [1.0, 0.0], [0.0, 3.0], [1.0, 3.0], 1.0),
            None
        );
    }
}
