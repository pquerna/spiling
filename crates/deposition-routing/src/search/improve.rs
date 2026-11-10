// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Local search over one tool group's order: segment relocation with optional
//! flip, in-place flip, loop reseaming and reversal of reversible runs, then
//! iterated kicks. Every move has an O(1) cost delta on modelled gap seconds
//! and respects prerequisites. Spent allowance simply stops the search.
use super::*;

const KICKS: usize = 48;
const SEGMENT: usize = 3;

pub(super) struct Order<'a, 't> {
    tasks: &'a [Task<'t>],
    travel: &'a Travel,
    members: Vec<u32>,
    start: Option<PointMm>,
    /// (local task, reverse, seam) in deposition order.
    visits: Vec<(u32, bool, u32)>,
    entry: Vec<PointMm>,
    exit: Vec<PointMm>,
    prerequisites: Vec<Vec<u32>>,
    dependents: Vec<Vec<u32>>,
}
impl<'a, 't> Order<'a, 't> {
    pub fn new(
        tasks: &'a [Task<'t>],
        travel: &'a Travel,
        members: &[u32],
        start: Option<PointMm>,
        visits: Vec<(u32, bool, u32)>,
        prerequisites: Vec<Vec<u32>>,
        dependents: Vec<Vec<u32>>,
    ) -> Self {
        let mut order = Self {
            tasks,
            travel,
            members: members.to_vec(),
            start,
            visits,
            entry: vec![],
            exit: vec![],
            prerequisites,
            dependents,
        };
        order.refresh();
        order
    }
    fn task(&self, position: usize) -> &Task<'t> {
        &self.tasks[self.members[self.visits[position].0 as usize] as usize]
    }
    fn refresh(&mut self) {
        (self.entry, self.exit) = (0..self.visits.len())
            .map(|i| {
                let (_, reverse, seam) = self.visits[i];
                entry_exit(
                    self.task(i),
                    Visit {
                        task: 0,
                        reverse,
                        seam,
                    },
                )
            })
            .unzip();
    }
    pub fn end(&self) -> Option<PointMm> {
        self.exit.last().copied()
    }
    pub fn into_visits(self, members: &[u32]) -> impl Iterator<Item = Visit> + '_ {
        self.visits.into_iter().map(|(l, reverse, seam)| Visit {
            task: members[l as usize],
            reverse,
            seam,
        })
    }
    fn between(&self, from: Option<PointMm>, to: PointMm) -> f64 {
        from.map_or(0., |from| gap_seconds(self.travel, from, to))
    }
    fn leaving(&self, position: Option<usize>) -> Option<PointMm> {
        position.map_or(self.start, |p| Some(self.exit[p]))
    }
    /// Modelled seconds from position `a`'s exit (or the start) to `b`'s entry.
    fn gap(&self, a: Option<usize>, b: usize) -> f64 {
        self.between(self.leaving(a), self.entry[b])
    }
    fn cost(&self) -> f64 {
        (0..self.visits.len())
            .map(|i| self.gap(i.checked_sub(1), i))
            .sum()
    }
    fn positions(&self) -> Vec<usize> {
        let mut at = vec![0; self.visits.len()];
        for (i, v) in self.visits.iter().enumerate() {
            at[v.0 as usize] = i;
        }
        at
    }
    fn reversible(&self, position: usize) -> bool {
        matches!(
            self.task(position).ends,
            Ends::Open {
                reversible: true,
                ..
            }
        )
    }
    /// Reversing a run keeps a loop as it is and flips a reversible open path.
    fn may_run_reverse(&self, position: usize) -> bool {
        self.reversible(position) || matches!(self.task(position).ends, Ends::Loop { .. })
    }
    /// Move [i, i+len) to the best permitted insertion point; a single
    /// reversible path may be inserted flipped. Returns the new position.
    fn try_move(&mut self, i: usize, len: usize, at: &[usize]) -> Option<usize> {
        let n = self.visits.len();
        let (first, last) = (i, i + len - 1);
        let prev = i.checked_sub(1);
        let next = (last + 1 < n).then_some(last + 1);
        let removed = self.gap(prev, first) + next.map_or(0., |k| self.gap(Some(last), k))
            - next.map_or(0., |k| self.gap(prev, k));
        let (mut low, mut high) = (0usize, n);
        for k in first..=last {
            let l = self.visits[k].0 as usize;
            for q in &self.prerequisites[l] {
                if at[*q as usize] < first {
                    low = low.max(at[*q as usize] + 1);
                }
            }
            for q in &self.dependents[l] {
                if at[*q as usize] > last {
                    high = high.min(at[*q as usize]);
                }
            }
        }
        let may_flip = len == 1 && self.reversible(first);
        let mut best = (1e-9, usize::MAX, false);
        for j in low..=high {
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
                let flipped = self.between(self.leaving(u), self.exit[first])
                    + v.map_or(0., |v| self.between(Some(self.entry[first]), self.entry[v]))
                    - base;
                if removed - flipped > best.0 {
                    best = (removed - flipped, j, true);
                }
            }
        }
        if best.1 == usize::MAX {
            return None;
        }
        let mut segment: Vec<_> = self.visits.drain(first..=last).collect();
        if best.2 {
            segment[0].1 = !segment[0].1;
        }
        let target = if best.1 > last { best.1 - len } else { best.1 };
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
    /// Flip a reversible open path, or move a loop's seam, when that is cheaper.
    fn try_entry(&mut self, i: usize) -> bool {
        let before = self.around(i);
        let original = self.visits[i];
        let mut best = (before - 1e-9, original);
        let candidates: Vec<(u32, bool, u32)> = match self.task(i).ends {
            Ends::Open { reversible, .. } if reversible => vec![(original.0, !original.1, 0)],
            Ends::Loop { seams } if seams.len() > 1 => (0..seams.len() as u32)
                .filter(|s| *s != original.2)
                .map(|s| (original.0, false, s))
                .collect(),
            _ => return false,
        };
        for candidate in candidates {
            self.visits[i] = candidate;
            let (entry, exit) = entry_exit(
                self.task(i),
                Visit {
                    task: 0,
                    reverse: candidate.1,
                    seam: candidate.2,
                },
            );
            (self.entry[i], self.exit[i]) = (entry, exit);
            let after = self.around(i);
            if after < best.0 {
                best = (after, candidate);
            }
        }
        self.visits[i] = best.1;
        self.refresh();
        best.1 != original
    }
    /// Reverse [i, k] when every member may reverse and none depends on another.
    fn try_reverse(&mut self, i: usize, at: &[usize]) -> Option<usize> {
        let n = self.visits.len();
        if !self.may_run_reverse(i) {
            return None;
        }
        let mut best = (1e-9, usize::MAX);
        let prev = self.leaving(i.checked_sub(1));
        for k in i + 1..n {
            let l = self.visits[k].0 as usize;
            if !self.may_run_reverse(k)
                || self.prerequisites[l]
                    .iter()
                    .any(|q| (i..k).contains(&at[*q as usize]))
            {
                break;
            }
            let after = (k + 1 < n).then(|| self.entry[k + 1]);
            let old = self.between(prev, self.entry[i])
                + after.map_or(0., |p| self.between(Some(self.exit[k]), p));
            let new = self.between(prev, self.exit[k])
                + after.map_or(0., |p| self.between(Some(self.entry[i]), p));
            if old - new > best.0 {
                best = (old - new, k);
            }
        }
        if best.1 == usize::MAX {
            return None;
        }
        self.visits[i..=best.1].reverse();
        for position in i..=best.1 {
            if self.reversible(position) {
                self.visits[position].1 = !self.visits[position].1;
            }
        }
        self.refresh();
        Some(best.1)
    }
    /// Relocate one random visit to a random prerequisite-feasible position.
    fn kick(&mut self, random: &mut u64) -> [usize; 2] {
        let n = self.visits.len();
        let mut next = || {
            *random ^= *random << 13;
            *random ^= *random >> 7;
            *random ^= *random << 17;
            *random
        };
        let i = (next() % n as u64) as usize;
        let at = self.positions();
        let l = self.visits[i].0 as usize;
        let low = self.prerequisites[l]
            .iter()
            .map(|q| at[*q as usize] + 1)
            .max()
            .unwrap_or(0);
        let high = self.dependents[l]
            .iter()
            .map(|q| at[*q as usize])
            .min()
            .unwrap_or(n);
        // Positions in the order with visit i removed.
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
    /// Don't-look-bit descent: only visits near a change are re-examined.
    /// Returns false when the allowance ran out.
    fn descend(&mut self, context: &mut Context<'_, '_>, seeds: &[usize]) -> Result<bool, Error> {
        let n = self.visits.len();
        let mut queued = vec![false; n];
        let mut queue = std::collections::VecDeque::new();
        let wake = |order: &Self,
                    around: usize,
                    queued: &mut Vec<bool>,
                    queue: &mut std::collections::VecDeque<u32>| {
            for k in around.saturating_sub(2)..(around + 3).min(n) {
                let l = order.visits[k].0;
                if !queued[l as usize] {
                    queued[l as usize] = true;
                    queue.push_back(l);
                }
            }
        };
        for &k in seeds {
            wake(self, k, &mut queued, &mut queue);
        }
        while let Some(l) = queue.pop_front() {
            if !context.charge(n as u64)? {
                return Ok(false);
            }
            let at = self.positions();
            let i = at[l as usize];
            let mut changed = None;
            for len in 1..=SEGMENT.min(n - i) {
                if let Some(t) = self.try_move(i, len, &at) {
                    changed = Some([t, t + len]);
                    break;
                }
            }
            if changed.is_none() && self.try_entry(i) {
                changed = Some([i, i]);
            }
            if changed.is_none()
                && let Some(k) = self.try_reverse(i, &at)
            {
                changed = Some([i, k]);
            }
            queued[l as usize] = false;
            if let Some(places) = changed {
                for k in [i, places[0], places[1]] {
                    wake(self, k.min(n - 1), &mut queued, &mut queue);
                }
            }
        }
        Ok(true)
    }
}

/// Descent, then iterated kicks keeping the best order. Cancellation aborts;
/// a spent allowance returns the best order reached.
pub(super) fn improve(
    context: &mut Context<'_, '_>,
    order: &mut Order<'_, '_>,
) -> Result<(), Error> {
    let n = order.visits.len();
    if n < 2 {
        return Ok(());
    }
    let all: Vec<usize> = (0..n).collect();
    let mut best = (order.cost(), order.visits.clone());
    let mut funded = order.descend(context, &all)?;
    let mut random = 0x2545_F491_4F6C_DD1D_u64;
    for _ in 0..KICKS {
        if order.cost() < best.0 - 1e-9 {
            best = (order.cost(), order.visits.clone());
        } else {
            order.visits.clone_from(&best.1);
            order.refresh();
        }
        if !funded {
            break;
        }
        let mut seeds = Vec::new();
        for _ in 0..3 {
            seeds.extend(order.kick(&mut random));
        }
        funded = order.descend(context, &seeds)?;
    }
    if order.cost() >= best.0 - 1e-9 {
        order.visits = best.1;
        order.refresh();
    }
    Ok(())
}
