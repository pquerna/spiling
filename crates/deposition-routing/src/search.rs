// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Editable scheduling algorithm. Its output is admitted only by the frozen
//! checker; nothing here can add, drop or alter a task.
mod construct;
mod improve;
mod sequence;
use crate::*;

pub(crate) struct Context<'a, 't> {
    pub tasks: &'a [Task<'t>],
    pub machine: &'a Machine,
    pub order: ToolOrder,
    pub cancel: &'a AtomicBool,
    pub work: u64,
    pub limit: u64,
}
impl Context<'_, '_> {
    /// False once the optional allowance is spent. Cancellation is an error.
    pub fn charge(&mut self, amount: u64) -> Result<bool, Error> {
        cancelled(self.cancel)?;
        if self.work.saturating_add(amount) > self.limit {
            return Ok(false);
        }
        self.work += amount;
        Ok(true)
    }
}

pub(crate) type Groups = Vec<(u32, u32, Range<usize>)>;

/// Tasks bucketed by barrier (ascending), then by tool.
struct Buckets {
    barriers: Vec<u32>,
    /// `members[barrier_index][tool]` holds task indexes in input order.
    members: Vec<Vec<Vec<u32>>>,
}
fn buckets(context: &Context<'_, '_>) -> Buckets {
    let mut barriers: Vec<u32> = context.tasks.iter().map(|t| t.barrier).collect();
    barriers.sort_unstable();
    barriers.dedup();
    let tools = context.machine.tools as usize;
    let mut members = vec![vec![Vec::new(); tools]; barriers.len()];
    for (index, task) in context.tasks.iter().enumerate() {
        let b = barriers.binary_search(&task.barrier).unwrap();
        members[b][task.tool as usize].push(index as u32);
    }
    Buckets { barriers, members }
}
fn present(context: &Context<'_, '_>, tools: &[Vec<u32>]) -> Vec<u32> {
    (0..context.machine.tools)
        .filter(|t| {
            context.machine.change.service == Service::EveryBarrier
                || !tools[*t as usize].is_empty()
        })
        .collect()
}

pub(crate) fn solve(context: &mut Context<'_, '_>) -> Result<(Vec<Visit>, Groups), Error> {
    let buckets = buckets(context);
    let orders = sequence::plan(context, &buckets.barriers, &buckets.members)?;
    let total: usize = orders.iter().map(Vec::len).sum();
    let mut visits = Vec::with_capacity(context.tasks.len());
    let mut groups = Vec::with_capacity(total);
    let mut position = context.machine.start;
    let mut local = vec![u32::MAX; context.tasks.len()];
    let mut remaining = total as u64;
    for (b, order) in orders.iter().enumerate() {
        for &tool in order {
            let members = &buckets.members[b][tool as usize];
            let begin = visits.len();
            if !members.is_empty() {
                let z = match context.tasks[members[0] as usize].ends {
                    Ends::Open { first, .. } => first[2],
                    Ends::Loop { seams } => seams[0][2],
                };
                let start = match context.machine.change.stations_xy[tool as usize] {
                    Some(xy) => Some([xy[0], xy[1], z]),
                    None => position,
                };
                for (l, index) in members.iter().enumerate() {
                    local[*index as usize] = l as u32;
                }
                // Spread what is left of the allowance over the groups left.
                let share = context.limit.saturating_sub(context.work) / remaining.max(1);
                let ceiling = context.work.saturating_add(share);
                let mut order = construct::nearest(context, members, &local, start)?;
                let limit = std::mem::replace(&mut context.limit, ceiling);
                let improved = improve::improve(context, &mut order);
                context.limit = limit;
                improved?;
                position = order.end().or(start);
                visits.extend(order.into_visits(members));
                for index in members {
                    local[*index as usize] = u32::MAX;
                }
            } else if let (Some(xy), Some(p)) =
                (context.machine.change.stations_xy[tool as usize], position)
            {
                position = Some([xy[0], xy[1], p[2]]);
            }
            remaining = remaining.saturating_sub(1);
            groups.push((buckets.barriers[b], tool, begin..visits.len()));
        }
    }
    Ok((visits, groups))
}

/// Input order within ascending tool groups: no search, entries as given.
pub(crate) fn input_order(context: &mut Context<'_, '_>) -> Result<(Vec<Visit>, Groups), Error> {
    let buckets = buckets(context);
    let mut visits = Vec::with_capacity(context.tasks.len());
    let mut groups = Vec::new();
    for (b, tools) in buckets.members.iter().enumerate() {
        cancelled(context.cancel)?;
        for tool in present(context, tools) {
            let begin = visits.len();
            visits.extend(tools[tool as usize].iter().map(|task| Visit {
                task: *task,
                reverse: false,
                seam: 0,
            }));
            groups.push((buckets.barriers[b], tool, begin..visits.len()));
        }
    }
    Ok((visits, groups))
}
