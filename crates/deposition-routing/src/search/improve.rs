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
    pricing: Pricing,
    members: Vec<u32>,
    start: Option<PointMm>,
    /// (local task, reverse, seam) in deposition order.
    visits: Vec<(u32, bool, u32)>,
    entry: Vec<PointMm>,
    exit: Vec<PointMm>,
    /// Position by local task, maintained with the changed visit range.
    at: Vec<usize>,
    /// Incoming gap seconds for each visit, using the same frozen pricing.
    incoming: Vec<f64>,
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
        let n = visits.len();
        let mut order = Self {
            tasks,
            pricing: Pricing::new(travel),
            members: members.to_vec(),
            start,
            visits,
            entry: vec![[0.; 3]; n],
            exit: vec![[0.; 3]; n],
            at: vec![0; n],
            incoming: vec![0.; n],
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
        self.refresh_range(0..self.visits.len());
    }
    /// Only moved visits change endpoints/positions. Their following gap also
    /// changes, even when that following visit itself stays in place.
    fn refresh_range(&mut self, range: Range<usize>) {
        for i in range.clone() {
            let (local, reverse, seam) = self.visits[i];
            self.at[local as usize] = i;
            (self.entry[i], self.exit[i]) = entry_exit(
                self.task(i),
                Visit {
                    task: 0,
                    reverse,
                    seam,
                },
            );
        }
        for i in range.start..range.end.saturating_add(1).min(self.visits.len()) {
            self.incoming[i] = self.gap(i.checked_sub(1), i);
        }
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
    #[inline(always)]
    fn between(&self, from: Option<PointMm>, to: PointMm) -> f64 {
        from.map_or(0., |from| self.pricing.seconds(from, to))
    }
    fn leaving(&self, position: Option<usize>) -> Option<PointMm> {
        position.map_or(self.start, |p| Some(self.exit[p]))
    }
    /// Modelled seconds from position `a`'s exit (or the start) to `b`'s entry.
    #[inline(always)]
    fn gap(&self, a: Option<usize>, b: usize) -> f64 {
        self.between(self.leaving(a), self.entry[b])
    }
    fn cost(&self) -> f64 {
        self.incoming.iter().sum()
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
    fn try_move(&mut self, i: usize, len: usize, to_first: &mut [Option<f64>]) -> Option<usize> {
        let n = self.visits.len();
        let (first, last) = (i, i + len - 1);
        let prev = i.checked_sub(1);
        let next = (last + 1 < n).then_some(last + 1);
        let removed = self.incoming[first] + next.map_or(0., |k| self.incoming[k])
            - next.map_or(0., |k| self.gap(prev, k));
        let (mut low, mut high) = (0usize, n);
        for k in first..=last {
            let l = self.visits[k].0 as usize;
            for q in &self.prerequisites[l] {
                if self.at[*q as usize] < first {
                    low = low.max(self.at[*q as usize] + 1);
                }
            }
            for q in &self.dependents[l] {
                if self.at[*q as usize] > last {
                    high = high.min(self.at[*q as usize]);
                }
            }
        }
        let may_flip = len == 1 && self.reversible(first);
        let mut best = (1e-9, usize::MAX, false);
        for (offset, cached) in to_first.get_mut(low..=high)?.iter_mut().enumerate() {
            let j = low + offset;
            if (first..=last + 1).contains(&j) {
                continue;
            }
            // j-1 is never inside the segment: those j are excluded above.
            let u = j.checked_sub(1);
            let v = (j < n).then_some(j);
            let base = v.map_or(0., |v| self.incoming[v]);
            // All segment lengths share their first visit. Reuse this side
            // of the insertion across lengths while the order is unchanged.
            let left = *cached.get_or_insert_with(|| self.gap(u, first));
            let added = left + v.map_or(0., |v| self.gap(Some(last), v)) - base;
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
        let target = if best.1 > last { best.1 - len } else { best.1 };
        if target > first {
            self.visits[first..best.1].rotate_left(len);
        } else {
            self.visits[best.1..=last].rotate_right(len);
        }
        if best.2 {
            self.visits[target].1 = !self.visits[target].1;
        }
        self.refresh_range(first.min(target)..(last + 1).max(target + len));
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
        let before = self.incoming[i] + self.incoming.get(i + 1).copied().unwrap_or(0.);
        let original = self.visits[i];
        let original_entry = self.entry[i];
        let original_exit = self.exit[i];
        let mut best = (before - 1e-9, original);
        let (count, reverse) = match self.task(i).ends {
            Ends::Open {
                reversible: true, ..
            } => (1, !original.1),
            Ends::Loop { seams } if seams.len() > 1 => (seams.len(), false),
            _ => return false,
        };
        for seam in 0..count {
            let candidate = (original.0, reverse, seam as u32);
            if candidate == original {
                continue;
            }
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
        if best.1 == original {
            self.entry[i] = original_entry;
            self.exit[i] = original_exit;
            false
        } else {
            self.refresh_range(i..i + 1);
            true
        }
    }
    /// Reverse [i, k] when every member may reverse and none depends on another.
    fn try_reverse(&mut self, i: usize) -> Option<usize> {
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
                    .any(|q| (i..k).contains(&self.at[*q as usize]))
            {
                break;
            }
            let after = (k + 1 < n).then(|| self.entry[k + 1]);
            let old = self.incoming[i] + self.incoming.get(k + 1).copied().unwrap_or(0.);
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
        self.refresh_range(i..best.1 + 1);
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
        let l = self.visits[i].0 as usize;
        let low = self.prerequisites[l]
            .iter()
            .map(|q| self.at[*q as usize] + 1)
            .max()
            .unwrap_or(0);
        let high = self.dependents[l]
            .iter()
            .map(|q| self.at[*q as usize])
            .min()
            .unwrap_or(n);
        // Positions in the order with visit i removed.
        let low = if low > i { low - 1 } else { low };
        let high = (if high > i { high - 1 } else { high }).min(n - 1);
        if high < low {
            return [i, i];
        }
        let j = low + (next() % (high - low + 1) as u64) as usize;
        if i < j {
            self.visits[i..=j].rotate_left(1);
        } else {
            self.visits[j..=i].rotate_right(1);
        }
        self.refresh_range(i.min(j)..i.max(j) + 1);
        [i, j]
    }
    /// Don't-look-bit descent: only visits near a change are re-examined.
    /// Returns false when the allowance ran out.
    fn descend(&mut self, context: &mut Context<'_, '_>, seeds: &[usize]) -> Result<bool, Error> {
        let n = self.visits.len();
        let mut queued = vec![false; n];
        let mut to_first = vec![None; n + 1];
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
            let i = self.at[l as usize];
            let mut changed = None;
            to_first.fill(None);
            for len in 1..=SEGMENT.min(n - i) {
                if let Some(t) = self.try_move(i, len, &mut to_first) {
                    changed = Some([t, t + len]);
                    break;
                }
            }
            if changed.is_none() && self.try_entry(i) {
                changed = Some([i, i]);
            }
            if changed.is_none()
                && let Some(k) = self.try_reverse(i)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_path_mutations_preserve_cached_state_and_prerequisites() {
        const N: usize = 19;
        let loops: Vec<[PointMm; 4]> = (0..N)
            .map(|i| {
                let x = (i % 5) as f64 * 1.7;
                let y = (i / 5) as f64 * 2.3;
                [
                    [x, y, 0.3],
                    [x + 0.8, y, 0.3],
                    [x + 0.8, y + 0.6, 0.3],
                    [x, y + 0.6, 0.3],
                ]
            })
            .collect();
        let tasks: Vec<Task<'_>> = (0..N)
            .map(|i| Task {
                ends: if i % 3 == 0 {
                    Ends::Loop { seams: &loops[i] }
                } else {
                    Ends::Open {
                        first: loops[i][0],
                        last: loops[i][2],
                        reversible: i % 5 != 0,
                    }
                },
                tool: 0,
                barrier: 0,
                prerequisites: &[],
                duration_s: 1.,
                seam_policy: None,
                physical_z_mm: [0., 0.3],
            })
            .collect();
        let members: Vec<u32> = (0..N as u32).rev().collect();
        let mut prerequisites = vec![Vec::new(); N];
        let mut dependents = vec![Vec::new(); N];
        for i in (4..N).step_by(4) {
            prerequisites[i].push((i - 3) as u32);
            dependents[i - 3].push(i as u32);
        }
        for start in [None, Some([-1., 2., 0.1])] {
            for retraction in [
                None,
                Some(Retraction {
                    minimum_mm: 2.,
                    seconds: 0.15,
                    lift_mm: 0.4,
                }),
            ] {
                let travel = Travel {
                    speed_mm_s: 120.,
                    lift_mm: 0.2,
                    retraction,
                };
                let initial: Vec<_> = (0..N as u32).map(|i| (i, false, 0)).collect();
                let mut order = Order::new(
                    &tasks,
                    &travel,
                    &members,
                    start,
                    initial.clone(),
                    prerequisites.clone(),
                    dependents.clone(),
                );
                let mut random = 0x2545_F491_4F6C_DD1D_u64;
                for step in 0..600 {
                    let i = step * 7 % N;
                    match step % 5 {
                        0 => {
                            order.try_move(i, (step % 3 + 1).min(N - i), &mut [None; N + 1]);
                        }
                        1 => {
                            order.try_entry(i);
                        }
                        2 => {
                            order.try_reverse(i);
                        }
                        3 => {
                            order.kick(&mut random);
                        }
                        _ => {
                            order.visits.clone_from(&initial);
                            order.refresh();
                        }
                    }
                    for (position, &(local, reverse, seam)) in order.visits.iter().enumerate() {
                        let expected = entry_exit(
                            &tasks[members[local as usize] as usize],
                            Visit {
                                task: 0,
                                reverse,
                                seam,
                            },
                        );
                        assert_eq!((order.entry[position], order.exit[position]), expected);
                        assert_eq!(order.at[local as usize], position);
                        assert_eq!(
                            order.incoming[position].to_bits(),
                            order.gap(position.checked_sub(1), position).to_bits()
                        );
                        assert!(
                            prerequisites[local as usize]
                                .iter()
                                .all(|p| order.at[*p as usize] < position)
                        );
                    }
                }
            }
        }
    }
}
