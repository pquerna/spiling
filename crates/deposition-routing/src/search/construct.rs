// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Nearest-entry construction for one tool group, honouring prerequisites.
//! Always completes: it is the schedule when no improvement allowance is left.
//! Ready entries sit in a uniform XY grid; each step takes the entry with the
//! least squared 3D distance, ties to the lowest offer (task, then entry), so
//! the result is the exact nearest choice whatever the grid's shape.
use super::improve::Order;
use super::*;

/// One permitted way into a task.
#[derive(Clone, Copy)]
struct Offer {
    local: u32,
    reverse: bool,
    seam: u32,
    entry: PointMm,
    exit: PointMm,
    /// Ranks the first pick when no position is known: the entry of an open
    /// path, the least vertex of a loop, so a policy that narrows a loop's
    /// seams does not change which task begins.
    first: PointMm,
}

/// Ready offers bucketed by XY cell, with a flat list of all of them.
struct Grid {
    min: [f64; 2],
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
    /// Offer id to (cell, index in cell, index in `live`), or `u32::MAX`.
    slot: Vec<[u32; 3]>,
    live: Vec<u32>,
}
impl Grid {
    fn new(offers: &[Offer]) -> Self {
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for offer in offers {
            for axis in 0..2 {
                low[axis] = low[axis].min(offer.entry[axis]);
                high[axis] = high[axis].max(offer.entry[axis]);
            }
        }
        let count = offers.len().max(1) as f64;
        let (w, h) = ((high[0] - low[0]).max(0.), (high[1] - low[1]).max(0.));
        // About one offer per cell; a degenerate extent still has one cell.
        let mut cell = (w * h / count).sqrt().max(w.max(h) / count);
        if !(cell.is_finite() && cell > 0.) {
            cell = 1.;
        }
        let nx = (w / cell) as usize + 1;
        let ny = (h / cell) as usize + 1;
        Self {
            min: if offers.is_empty() { [0.; 2] } else { low },
            cell,
            nx,
            ny,
            cells: vec![Vec::new(); nx * ny],
            slot: vec![[u32::MAX; 3]; offers.len()],
            live: Vec::new(),
        }
    }
    fn coordinate(&self, value: f64, axis: usize) -> usize {
        let limit = if axis == 0 { self.nx } else { self.ny };
        let c = ((value - self.min[axis]) / self.cell).floor();
        if c >= 0. {
            (c as usize).min(limit - 1)
        } else {
            0
        }
    }
    fn insert(&mut self, id: u32, offer: &Offer) {
        let c = self.coordinate(offer.entry[1], 1) * self.nx + self.coordinate(offer.entry[0], 0);
        self.slot[id as usize] = [c as u32, self.cells[c].len() as u32, self.live.len() as u32];
        self.cells[c].push(id);
        self.live.push(id);
    }
    fn remove(&mut self, id: u32) {
        let [c, at, flat] = std::mem::replace(&mut self.slot[id as usize], [u32::MAX; 3]);
        let cell = &mut self.cells[c as usize];
        cell.swap_remove(at as usize);
        if let Some(&moved) = cell.get(at as usize) {
            self.slot[moved as usize][1] = at;
        }
        self.live.swap_remove(flat as usize);
        if let Some(&moved) = self.live.get(flat as usize) {
            self.slot[moved as usize][2] = flat;
        }
    }
    /// The live offer nearest `from`, ties to the lowest id.
    fn nearest(&self, offers: &[Offer], from: PointMm) -> Option<u32> {
        let key = |id: u32| {
            let e = offers[id as usize].entry;
            let (dx, dy, dz) = (e[0] - from[0], e[1] - from[1], e[2] - from[2]);
            (dx * dx + dy * dy + dz * dz, id)
        };
        let better = |a: (f64, u32), b: Option<(f64, u32)>| {
            b.is_none_or(|b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).is_lt())
        };
        let mut best: Option<(f64, u32)> = None;
        let (cx, cy) = (self.coordinate(from[0], 0), self.coordinate(from[1], 1));
        let mut examined = 0usize;
        for r in 0.. {
            if examined > self.live.len() {
                // Sparse remainder: the flat list is cheaper than more rings.
                best = None;
                for &id in &self.live {
                    let k = key(id);
                    if better(k, best) {
                        best = Some(k);
                    }
                }
                break;
            }
            let mut visit = |x: usize, y: usize, best: &mut Option<(f64, u32)>| {
                for &id in &self.cells[y * self.nx + x] {
                    let k = key(id);
                    if better(k, *best) {
                        *best = Some(k);
                    }
                }
                examined += 1;
            };
            // Ring r: the cells at Chebyshev distance r, clipped to the grid.
            let (x0, x1) = (cx.saturating_sub(r), (cx + r).min(self.nx - 1));
            let (y0, y1) = (cy.saturating_sub(r), (cy + r).min(self.ny - 1));
            for y in [cy.checked_sub(r), Some(cy + r)]
                .into_iter()
                .flatten()
                .filter(|y| *y < self.ny)
                .take(if r == 0 { 1 } else { 2 })
            {
                for x in x0..=x1 {
                    visit(x, y, &mut best);
                }
            }
            if r > 0 {
                for y in (cy.saturating_sub(r - 1)).max(y0)..=(cy + r - 1).min(y1) {
                    for x in [cx.checked_sub(r), Some(cx + r)]
                        .into_iter()
                        .flatten()
                        .filter(|x| *x < self.nx)
                    {
                        visit(x, y, &mut best);
                    }
                }
            }
            // Unexamined offers lie outside this block; sides on the grid's
            // edge have nothing beyond them.
            let mut bound = f64::INFINITY;
            let mut open = false;
            let edge = |c: usize, axis: usize| self.min[axis] + c as f64 * self.cell;
            for (side, beyond) in [
                (cx > r, from[0] - edge(cx.saturating_sub(r), 0)),
                (cx + r + 1 < self.nx, edge(cx + r + 1, 0) - from[0]),
                (cy > r, from[1] - edge(cy.saturating_sub(r), 1)),
                (cy + r + 1 < self.ny, edge(cy + r + 1, 1) - from[1]),
            ] {
                if side {
                    open = true;
                    bound = bound.min(beyond);
                }
            }
            if !open {
                break;
            }
            // Cell assignment rounds; keep the bound conservative.
            let bound = (bound - self.cell * 1e-6).max(0.);
            if best.is_some_and(|b| bound * bound > b.0) {
                break;
            }
        }
        best.map(|b| b.1)
    }
    /// The live offer with the least `first` coordinates, then the least
    /// entry, ties to the lowest id.
    fn least(&self, offers: &[Offer]) -> Option<u32> {
        self.live.iter().copied().min_by(|a, b| {
            let (p, q) = (&offers[*a as usize], &offers[*b as usize]);
            lex(p.first, q.first)
                .then_with(|| lex(p.entry, q.entry))
                .then(a.cmp(b))
        })
    }
}

fn lex(p: PointMm, q: PointMm) -> std::cmp::Ordering {
    p[0].total_cmp(&q[0])
        .then_with(|| p[1].total_cmp(&q[1]))
        .then_with(|| p[2].total_cmp(&q[2]))
}

pub(super) fn nearest<'a, 't>(
    context: &mut Context<'a, 't>,
    members: &[u32],
    start: Option<PointMm>,
) -> Result<Order<'a, 't>, Error> {
    let tasks = context.tasks;
    let travel = &context.machine.travel;
    let n = members.len();
    // Prerequisites inside this group; others are met by group order.
    let mut waiting = vec![0u32; n];
    let mut dependents = vec![Vec::new(); n];
    let mut prerequisites = vec![Vec::new(); n];
    for (l, index) in members.iter().enumerate() {
        for q in tasks[*index as usize].prerequisites {
            // Members ascend by task index.
            if let Ok(other) = members.binary_search(q) {
                waiting[l] += 1;
                dependents[other].push(l as u32);
                prerequisites[l].push(other as u32);
            }
        }
    }
    // Every permitted entry, grouped by task in ascending order.
    let mut seams = Vec::with_capacity(n);
    let mut offers = Vec::new();
    let mut first = Vec::with_capacity(n + 1);
    for (l, index) in members.iter().enumerate() {
        cancelled(context.cancel)?;
        let task = &tasks[*index as usize];
        first.push(offers.len() as u32);
        let l = l as u32;
        match task.ends {
            Ends::Open {
                first,
                last,
                reversible,
            } => {
                offers.push(Offer {
                    local: l,
                    reverse: false,
                    seam: 0,
                    entry: first,
                    exit: last,
                    first,
                });
                if reversible {
                    offers.push(Offer {
                        local: l,
                        reverse: true,
                        seam: 0,
                        entry: last,
                        exit: first,
                        first: last,
                    });
                }
            }
            Ends::Loop { seams: vertices } => {
                let permitted = crate::seam::policy_seams(task);
                let least = vertices
                    .iter()
                    .copied()
                    .fold(vertices[0], |a, b| if lex(b, a).is_lt() { b } else { a });
                let mut offer = |seam: u32| {
                    let p = vertices[seam as usize];
                    offers.push(Offer {
                        local: l,
                        reverse: false,
                        seam,
                        entry: p,
                        exit: p,
                        first: least,
                    });
                };
                match &permitted {
                    Some(only) => only.iter().copied().for_each(&mut offer),
                    None => (0..vertices.len() as u32).for_each(&mut offer),
                }
                seams.push((l, permitted));
                continue;
            }
        }
        seams.push((l, None));
    }
    first.push(offers.len() as u32);
    let mut grid = Grid::new(&offers);
    for l in 0..n {
        if waiting[l] == 0 {
            for id in first[l]..first[l + 1] {
                grid.insert(id, &offers[id as usize]);
            }
        }
    }
    let mut visits = Vec::with_capacity(n);
    let mut position = start;
    for _ in 0..n {
        cancelled(context.cancel)?;
        let chosen = match position {
            Some(from) => grid.nearest(&offers, from),
            None => grid.least(&offers),
        };
        // A prerequisite cycle inside the group leaves nothing ready.
        let offer = offers[chosen.ok_or(Error::Inconsistent)? as usize];
        let l = offer.local as usize;
        for id in first[l]..first[l + 1] {
            grid.remove(id);
        }
        for d in &dependents[l] {
            let d = *d as usize;
            waiting[d] -= 1;
            if waiting[d] == 0 {
                for id in first[d]..first[d + 1] {
                    grid.insert(id, &offers[id as usize]);
                }
            }
        }
        visits.push((offer.local, offer.reverse, offer.seam));
        position = Some(offer.exit);
    }
    let permitted = seams.into_iter().map(|(_, p)| p).collect();
    Ok(Order::new(
        tasks,
        travel,
        members,
        start,
        visits,
        prerequisites,
        dependents,
    )
    .with_seams(permitted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_nearest_matches_a_full_scan_through_removals() {
        let mut random = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = || {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            (random % 10_000) as f64 / 100.
        };
        // Clustered, duplicated and collinear entries all occur in layers.
        for shape in 0..3 {
            let offers: Vec<Offer> = (0..400)
                .map(|i| {
                    let p = match shape {
                        0 => [next(), next(), 0.2],
                        1 => [next().round(), (next() / 10.).round(), 0.2],
                        _ => [next(), 5., 0.2 + f64::from(i % 2) * 0.1],
                    };
                    Offer {
                        local: i,
                        reverse: false,
                        seam: 0,
                        entry: p,
                        exit: p,
                        first: p,
                    }
                })
                .collect();
            let mut grid = Grid::new(&offers);
            for (id, offer) in offers.iter().enumerate() {
                grid.insert(id as u32, offer);
            }
            for step in 0..offers.len() {
                let from = if step % 7 == 0 {
                    [-50. + next(), 300. - next(), 0.2]
                } else {
                    [next(), next(), 0.2]
                };
                let scan = grid.live.iter().copied().min_by(|a, b| {
                    let d = |id: u32| {
                        let e = offers[id as usize].entry;
                        let (x, y, z) = (e[0] - from[0], e[1] - from[1], e[2] - from[2]);
                        x * x + y * y + z * z
                    };
                    d(*a).total_cmp(&d(*b)).then(a.cmp(b))
                });
                let found = grid.nearest(&offers, from);
                assert_eq!(found, scan, "shape {shape} step {step}");
                grid.remove(found.unwrap());
            }
            assert!(grid.nearest(&offers, [0.; 3]).is_none());
        }
    }
}
