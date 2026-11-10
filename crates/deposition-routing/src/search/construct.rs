// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Nearest-entry construction for one tool group, honouring prerequisites.
//! Always completes: it is the schedule when no improvement allowance is left.
use super::improve::Order;
use super::*;

pub(super) fn nearest<'a, 't>(
    context: &mut Context<'a, 't>,
    members: &[u32],
    local: &[u32],
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
            let other = local[*q as usize];
            if other != u32::MAX {
                waiting[l] += 1;
                dependents[other as usize].push(l as u32);
                prerequisites[l].push(other);
            }
        }
    }
    let mut done = vec![false; n];
    let mut visits = Vec::with_capacity(n);
    let mut position = start;
    for _ in 0..n {
        cancelled(context.cancel)?;
        let mut best: Option<(f64, usize, bool, u32, PointMm, PointMm)> = None;
        for l in 0..n {
            if done[l] || waiting[l] != 0 {
                continue;
            }
            let mut offer = |reverse: bool, seam: u32, entry: PointMm, exit: PointMm| {
                let seconds = position.map_or(0., |from| gap_seconds(travel, from, entry));
                if best.is_none_or(|b| {
                    seconds
                        .total_cmp(&b.0)
                        .then_with(|| {
                            if position.is_none() {
                                entry[0]
                                    .total_cmp(&b.4[0])
                                    .then_with(|| entry[1].total_cmp(&b.4[1]))
                                    .then_with(|| entry[2].total_cmp(&b.4[2]))
                            } else {
                                std::cmp::Ordering::Equal
                            }
                        })
                        .is_lt()
                }) {
                    best = Some((seconds, l, reverse, seam, entry, exit));
                }
            };
            match tasks[members[l] as usize].ends {
                Ends::Open {
                    first,
                    last,
                    reversible,
                } => {
                    offer(false, 0, first, last);
                    if reversible {
                        offer(true, 0, last, first);
                    }
                }
                Ends::Loop { seams } => {
                    for (seam, vertex) in seams.iter().enumerate() {
                        offer(false, seam as u32, *vertex, *vertex);
                    }
                }
            }
        }
        // A prerequisite cycle inside the group leaves nothing ready.
        let (_, l, reverse, seam, _, exit) = best.ok_or(Error::Inconsistent)?;
        done[l] = true;
        for d in &dependents[l] {
            waiting[*d as usize] -= 1;
        }
        visits.push((l as u32, reverse, seam));
        position = Some(exit);
    }
    Ok(Order::new(
        tasks,
        travel,
        members,
        start,
        visits,
        prerequisites,
        dependents,
    ))
}
