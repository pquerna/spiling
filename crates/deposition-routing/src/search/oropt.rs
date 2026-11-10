// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Or-opt over an admitted incumbent, screened on straight entry/exit
//! distances with O(1) move deltas and explicit precedence checks. The
//! improved order is rebuilt through checked steps once; if it is not
//! admitted or not better, the incumbent is returned unchanged.
use crate::{
    planner::{Session, State},
    *,
};
type Visit = (usize, bool, usize);
fn terminals(p: &Problem, (stroke, reverse, seam): Visit) -> (PointMm, PointMm) {
    let edges = traversal(&p.strokes[stroke], reverse, seam);
    (edges[0].0, edges.last().unwrap().1)
}
/// None: not admitted. Resource/cancel propagate to the caller's policy.
pub(super) fn rebuild(session: &mut Session<'_>, order: &[Visit]) -> Result<Option<State>, Error> {
    let mut state = session.empty();
    for &(stroke, reverse, seam) in order {
        let entry = terminals(session.problem, (stroke, reverse, seam)).0;
        let mut travel_mm = session.travel(&state, entry, &[])?;
        if travel_mm.is_none() {
            let portals = super::portals::propose(session, &state)?;
            travel_mm = session.travel(&state, entry, &portals)?;
        }
        let Some(travel_mm) = travel_mm else {
            return Ok(None);
        };
        let mut admitted = false;
        for continuous in [travel_mm.len() == 1, false] {
            let step = Step {
                stroke,
                reverse,
                seam,
                travel_mm: travel_mm.clone(),
                continuous,
            };
            match session.push(&mut state, step) {
                Ok(()) => {
                    admitted = true;
                    break;
                }
                Err(Error::InvalidInput) => {}
                Err(other) => return Err(other),
            }
        }
        if !admitted {
            return Ok(None);
        }
    }
    Ok(Some(state))
}
pub(super) struct Order<'a> {
    p: &'a Problem,
    pub visits: Vec<Visit>,
    entry: Vec<PointMm>,
    exit: Vec<PointMm>,
    prerequisites: Vec<Vec<usize>>,
    dependents: Vec<Vec<usize>>,
    /// Keep physical bands in non-decreasing order: moves stay inside a band.
    pub band_major: bool,
}
impl<'a> Order<'a> {
    pub fn new(p: &'a Problem, route: &Route) -> Self {
        let index = |id: StrokeId| p.strokes.iter().position(|s| s.id == id).unwrap();
        let prerequisites: Vec<Vec<usize>> = p
            .strokes
            .iter()
            .map(|s| s.prerequisites.iter().map(|id| index(*id)).collect())
            .collect();
        let mut dependents = vec![vec![]; p.strokes.len()];
        for (i, before) in prerequisites.iter().enumerate() {
            for j in before {
                dependents[*j].push(i);
            }
        }
        let mut order = Self {
            p,
            visits: route
                .steps
                .iter()
                .map(|s| (s.stroke, s.reverse, s.seam))
                .collect(),
            entry: vec![],
            exit: vec![],
            prerequisites,
            dependents,
            band_major: true,
        };
        order.refresh();
        order
    }
    pub fn refresh(&mut self) {
        (self.entry, self.exit) = self.visits.iter().map(|v| terminals(self.p, *v)).unzip();
    }
    /// Straight distance from position `a`'s exit (or start) to `b`'s entry.
    fn gap(&self, a: Option<usize>, b: usize) -> f64 {
        a.map_or(self.p.start_mm, |a| self.exit[a])
            .distance(self.entry[b])
    }
    fn band(&self, position: usize) -> f64 {
        self.p.strokes[self.visits[position].0].physical_z_mm[1]
    }
    /// Insertion positions [start, end] of the contiguous band holding `i`.
    fn block(&self, i: usize) -> (usize, usize) {
        let b = self.band(i);
        let start = (0..=i)
            .rev()
            .take_while(|k| self.band(*k) == b)
            .last()
            .unwrap_or(i);
        let end = (i..self.visits.len())
            .take_while(|k| self.band(*k) == b)
            .last()
            .unwrap_or(i)
            + 1;
        (start, end)
    }
    pub fn cost(&self) -> f64 {
        (0..self.visits.len())
            .map(|i| self.gap(i.checked_sub(1), i))
            .sum()
    }
    fn position(&self) -> Vec<usize> {
        let mut at = vec![0; self.visits.len()];
        for (i, v) in self.visits.iter().enumerate() {
            at[v.0] = i;
        }
        at
    }
    /// Move [i, i+len) to sit between positions `u` and `u+1` of the order
    /// with that segment removed. Returns true when applied.
    fn try_move(&mut self, i: usize, len: usize, at: &[usize]) -> Option<usize> {
        let n = self.visits.len();
        let (first, last) = (i, i + len - 1);
        let prev = i.checked_sub(1);
        let next = (last + 1 < n).then_some(last + 1);
        let removed = self.gap(prev, first) + next.map_or(0., |k| self.gap(Some(last), k))
            - next.map_or(0., |k| self.gap(prev, k));
        // Segment members' precedence window in current positions.
        let mut low = 0usize;
        let mut high = n;
        for k in first..=last {
            let s = self.visits[k].0;
            for &q in &self.prerequisites[s] {
                if at[q] < first {
                    low = low.max(at[q] + 1);
                }
            }
            for &q in &self.dependents[s] {
                if at[q] > last {
                    high = high.min(at[q]);
                }
            }
        }
        if self.band_major {
            if (first..=last).any(|k| self.band(k) != self.band(first)) {
                return None;
            }
            let (start, end) = self.block(first);
            low = low.max(start);
            high = high.min(end);
        }
        let mut best = (1e-9, usize::MAX, false);
        // A single reversible open stroke may also be inserted flipped.
        let stroke = &self.p.strokes[self.visits[first].0];
        let may_flip = len == 1 && stroke.allow_reverse && !stroke.allow_seam_rotation;
        let at_point = |a: Option<usize>| a.map_or(self.p.start_mm, |a| self.exit[a]);
        // Insert before original position `j` (j outside the segment).
        for j in low..=high.min(n) {
            if (first..=last + 1).contains(&j) {
                continue;
            }
            // j-1 is never inside the segment: those j are excluded above.
            let u = j.checked_sub(1);
            let v = (j < n).then_some(j);
            let base = v.map_or(0., |v| self.gap(u, v));
            let added = self.gap(u, first) + v.map_or(0., |v| self.gap(Some(last), v)) - base;
            if removed - added > best.0 {
                best = (removed - added, j, false);
            }
            if may_flip {
                let flipped = at_point(u).distance(self.exit[first])
                    + v.map_or(0., |v| self.entry[first].distance(self.entry[v]))
                    - base;
                if removed - flipped > best.0 {
                    best = (removed - flipped, j, true);
                }
            }
        }
        if best.1 == usize::MAX {
            return None;
        }
        let j = best.1;
        let mut segment: Vec<Visit> = self.visits.drain(first..=last).collect();
        if best.2 {
            segment[0].1 = !segment[0].1;
        }
        let target = if j > last { j - len } else { j };
        self.visits.splice(target..target, segment);
        self.refresh();
        Some(target)
    }
    fn around(&self, i: usize) -> f64 {
        let after = if i + 1 < self.visits.len() {
            self.gap(Some(i), i + 1)
        } else {
            0.
        };
        self.gap(i.checked_sub(1), i) + after
    }
    /// Flip one reversible open stroke in place when that is shorter.
    fn try_flip(&mut self, i: usize) -> bool {
        let (s, reverse, seam) = self.visits[i];
        let stroke = &self.p.strokes[s];
        if !stroke.allow_reverse || stroke.allow_seam_rotation {
            return false;
        }
        let before = self.around(i);
        self.visits[i] = (s, !reverse, seam);
        self.refresh();
        let after = self.around(i);
        if after < before - 1e-9 {
            return true;
        }
        self.visits[i] = (s, reverse, seam);
        self.refresh();
        false
    }
    /// Reverse [i, k] when every member may reverse and none depends on another.
    fn try_reverse(&mut self, i: usize, at: &[usize]) -> Option<usize> {
        let n = self.visits.len();
        let mut best = (1e-9, usize::MAX);
        for k in i + 1..n {
            let s = &self.p.strokes[self.visits[k].0];
            if (self.band_major && self.band(k) != self.band(i))
                || !s.allow_reverse
                || s.allow_seam_rotation
                || self.prerequisites[self.visits[k].0]
                    .iter()
                    .any(|q| (i..k).contains(&at[*q]))
            {
                break;
            }
            let prev = i.checked_sub(1).map_or(self.p.start_mm, |u| self.exit[u]);
            let old = prev.distance(self.entry[i])
                + if k + 1 < n {
                    self.exit[k].distance(self.entry[k + 1])
                } else {
                    0.
                };
            let new = prev.distance(self.exit[k])
                + if k + 1 < n {
                    self.entry[i].distance(self.entry[k + 1])
                } else {
                    0.
                };
            if old - new > best.0 {
                best = (old - new, k);
            }
        }
        let first = &self.p.strokes[self.visits[i].0];
        if best.1 == usize::MAX || !first.allow_reverse || first.allow_seam_rotation {
            return None;
        }
        self.visits[i..=best.1].reverse();
        for v in &mut self.visits[i..=best.1] {
            v.1 = !v.1;
        }
        self.refresh();
        Some(best.1)
    }
    /// Relocate one random visit to a random precedence-feasible position.
    /// Returns the disturbed positions.
    fn kick(&mut self, random: &mut u64) -> [usize; 2] {
        let n = self.visits.len();
        let mut next = || {
            *random ^= *random << 13;
            *random ^= *random >> 7;
            *random ^= *random << 17;
            *random
        };
        let i = (next() % n as u64) as usize;
        let at = self.position();
        let s = self.visits[i].0;
        let low = self.prerequisites[s]
            .iter()
            .map(|q| at[*q] + 1)
            .max()
            .unwrap_or(0);
        let high = self.dependents[s].iter().map(|q| at[*q]).min().unwrap_or(n);
        let (low, high) = if self.band_major {
            let (start, end) = self.block(i);
            (low.max(start), high.min(end))
        } else {
            (low, high)
        };
        // Positions in the order with visit i removed, respecting precedence.
        let low = if low > i { low - 1 } else { low };
        let high = (if high > i { high - 1 } else { high }).min(n - 1);
        if high < low {
            return [i, i];
        }
        let j = low + (next() % (high - low + 1) as u64) as usize;
        let visit = self.visits.remove(i);
        self.visits.insert(j, visit);
        self.refresh();
        [i, j]
    }
    /// Don't-look-bit descent: only strokes near a change are re-examined.
    pub fn descend(
        &mut self,
        session: &mut Session<'_>,
        segment: usize,
        seeds: &[usize],
    ) -> Result<(), Error> {
        let n = self.visits.len();
        let mut queued = vec![false; self.p.strokes.len()];
        let mut queue = std::collections::VecDeque::new();
        let wake = |order: &Self,
                    around: usize,
                    queued: &mut Vec<bool>,
                    queue: &mut std::collections::VecDeque<usize>| {
            for k in around.saturating_sub(2)..(around + 3).min(n) {
                let s = order.visits[k].0;
                if !queued[s] {
                    queued[s] = true;
                    queue.push_back(s);
                }
            }
        };
        for &k in seeds {
            wake(self, k, &mut queued, &mut queue);
        }
        while let Some(s) = queue.pop_front() {
            session.charge(n as u64)?;
            let at = self.position();
            let i = at[s];
            let mut changed = None;
            for len in 1..=segment.min(n - i) {
                if let Some(t) = self.try_move(i, len, &at) {
                    changed = Some([t, t + len]);
                    break;
                }
            }
            if changed.is_none() && self.try_flip(i) {
                changed = Some([i, i]);
            }
            if changed.is_none()
                && let Some(k) = self.try_reverse(i, &at)
            {
                changed = Some([i, k]);
            }
            queued[s] = false;
            if let Some(places) = changed {
                for k in [i, places[0], places[1]] {
                    wake(self, k.min(n - 1), &mut queued, &mut queue);
                }
            }
        }
        Ok(())
    }
}
const KICKS: usize = 48;
const CROSS_KICKS: usize = 16;
fn checked_cost(route: &Route) -> (f64, usize) {
    let travel = route
        .steps
        .iter()
        .map(|s| {
            s.travel_mm
                .windows(2)
                .map(|w| w[0].distance(w[1]))
                .sum::<f64>()
        })
        .sum();
    (travel, route.steps.iter().filter(|s| !s.continuous).count())
}
/// One descent + ILS stage, rebuilt once; the incumbent survives any refusal.
fn stage(
    session: &mut Session<'_>,
    incumbent: Route,
    band_major: bool,
    kicks: usize,
) -> Result<Route, Error> {
    let p = session.problem;
    let mut order = Order::new(p, &incumbent);
    order.band_major = band_major;
    let before = order.cost();
    let all: Vec<usize> = (0..order.visits.len()).collect();
    match order.descend(session, 3, &all) {
        Err(Error::Cancelled) => return Err(Error::Cancelled),
        Err(_) => return Ok(incumbent),
        Ok(()) => {}
    }
    // Iterated local search: small random kicks, re-descend, keep the best.
    let mut best = (order.cost(), order.visits.clone());
    let mut random = 0x2545_F491_4F6C_DD1D_u64;
    for _ in 0..kicks {
        let mut seeds = Vec::new();
        for _ in 0..3 {
            seeds.extend(order.kick(&mut random));
        }
        match order.descend(session, 3, &seeds) {
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(_) => break,
            Ok(()) => {}
        }
        if order.cost() < best.0 - 1e-9 {
            best = (order.cost(), order.visits.clone());
        } else {
            order.visits.clone_from(&best.1);
            order.refresh();
        }
    }
    order.visits = best.1;
    order.refresh();
    if order.cost() >= before - 1e-9 {
        return Ok(incumbent);
    }
    let reference = checked_cost(&incumbent);
    match rebuild(session, &order.visits) {
        Ok(Some(state)) if state.travel_mm < reference.0 - 1e-9 && state.starts <= reference.1 => {
            Ok(state.route())
        }
        Err(Error::Cancelled) => Err(Error::Cancelled),
        _ => Ok(incumbent),
    }
}
/// Band-local search first; then cross-band moves, kept only if the checked
/// rebuild admits them. Exhausted work keeps the incumbent; cancel aborts.
pub(super) fn improve(session: &mut Session<'_>, incumbent: Route) -> Result<Route, Error> {
    let banded = stage(session, incumbent, true, KICKS)?;
    stage(session, banded, false, CROSS_KICKS)
}
