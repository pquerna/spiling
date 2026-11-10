// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Tool order per barrier. Free order minimizes tool selections exactly over
//! the whole barrier sequence; ties resolve to the lexicographically smallest
//! deterministic order so results do not depend on search effort.
use super::*;

/// Largest tool count whose orders are enumerated; beyond it order is fixed.
const ENUMERATED_TOOLS: usize = 6;

fn permutations(tools: &[u32], out: &mut Vec<Vec<u32>>, current: &mut Vec<u32>, used: &mut [bool]) {
    if current.len() == tools.len() {
        out.push(current.clone());
        return;
    }
    for i in 0..tools.len() {
        if !used[i] {
            used[i] = true;
            current.push(tools[i]);
            permutations(tools, out, current, used);
            current.pop();
            used[i] = false;
        }
    }
}

pub(super) fn plan(
    context: &mut Context<'_, '_>,
    barriers: &[u32],
    members: &[Vec<Vec<u32>>],
) -> Result<Vec<Vec<u32>>, Error> {
    let tools = context.machine.tools as usize;
    // Prerequisites that cross tools inside one barrier constrain tool order;
    // a prerequisite in a later barrier can never be met.
    let mut before = vec![vec![false; tools * tools]; barriers.len()];
    for task in context.tasks {
        let b = barriers.binary_search(&task.barrier).unwrap();
        for q in task.prerequisites {
            let other = &context.tasks[*q as usize];
            if other.barrier > task.barrier {
                return Err(Error::Inconsistent);
            }
            if other.barrier == task.barrier && other.tool != task.tool {
                before[b][other.tool as usize * tools + task.tool as usize] = true;
            }
        }
    }
    // Per barrier, per final tool: fewest selections so far and the order used.
    let mut frontier: Vec<(Option<u32>, u32, usize)> = vec![(context.machine.initial_tool, 0, 0)];
    let mut history: Vec<Vec<(Vec<u32>, usize)>> = Vec::with_capacity(barriers.len());
    for (b, groups) in members.iter().enumerate() {
        cancelled(context.cancel)?;
        let present = present(context, groups);
        let mut orders = Vec::new();
        if context.order == ToolOrder::Free && present.len() <= ENUMERATED_TOOLS {
            permutations(
                &present,
                &mut orders,
                &mut Vec::new(),
                &mut vec![false; present.len()],
            );
        } else if context.order == ToolOrder::Free {
            // Do not misclassify a valid larger inventory as inconsistent just
            // because exact enumeration is deliberately bounded.
            let mut remaining = present.clone();
            let mut order = Vec::with_capacity(remaining.len());
            while !remaining.is_empty() {
                cancelled(context.cancel)?;
                let index = remaining
                    .iter()
                    .position(|candidate| {
                        remaining
                            .iter()
                            .all(|other| !before[b][*other as usize * tools + *candidate as usize])
                    })
                    .ok_or(Error::Inconsistent)?;
                order.push(remaining.remove(index));
            }
            orders.push(order);
        } else {
            orders.push(present.clone());
        }
        orders.retain(|order| {
            order.iter().enumerate().all(|(i, later)| {
                order[..i]
                    .iter()
                    .all(|earlier| !before[b][*later as usize * tools + *earlier as usize])
            })
        });
        if orders.is_empty() {
            return Err(Error::Inconsistent);
        }
        let mut next: Vec<(Option<u32>, u32, usize)> = Vec::new();
        let mut chosen: Vec<(Vec<u32>, usize)> = Vec::new();
        for order in orders {
            // Every barrier comes from a task, so some tool is present.
            let (first, last) = (order[0], *order.last().unwrap());
            let best = frontier
                .iter()
                .enumerate()
                .map(|(state, (tool, selections, _))| {
                    let count = order.len() as u32 - u32::from(*tool == Some(first));
                    (selections + count, state)
                })
                .min()
                .unwrap();
            match next.iter().position(|(tool, _, _)| *tool == Some(last)) {
                Some(slot) if (next[slot].1, &chosen[next[slot].2].0) <= (best.0, &order) => {}
                Some(slot) => {
                    next[slot].1 = best.0;
                    chosen[next[slot].2] = (order, best.1);
                }
                None => {
                    next.push((Some(last), best.0, chosen.len()));
                    chosen.push((order, best.1));
                }
            }
        }
        frontier = next;
        history.push(chosen);
    }
    let mut state = frontier
        .iter()
        .min_by_key(|(tool, selections, _)| (*selections, *tool))
        .map(|(_, _, slot)| *slot);
    let mut result = vec![Vec::new(); barriers.len()];
    for b in (0..barriers.len()).rev() {
        let (order, previous) = &history[b][state.unwrap()];
        result[b] = order.clone();
        // Frontier entries and their history slots share one index.
        state = (b > 0).then_some(*previous);
    }
    Ok(result)
}
