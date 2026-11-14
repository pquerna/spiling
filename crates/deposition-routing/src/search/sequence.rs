// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Tool order per barrier. Free order minimizes tool selections exactly over
//! the whole barrier sequence; ties resolve to the lexicographically smallest
//! deterministic order so results do not depend on search effort.
use super::*;

/// Selections depend only on an order's first and last tool, so per pair of
/// a source `first` and a sink `last` the lexicographically smallest valid
/// order is the only candidate any tie-break can choose: `first`, then the
/// greedy smallest-available topological order of the rest, then `last`.
/// This is exact for every tool count, with no enumeration ceiling.
fn candidate_orders(present: &[u32], precedes: impl Fn(u32, u32) -> bool) -> Vec<Vec<u32>> {
    let mut orders = Vec::new();
    for &first in present {
        if present.iter().any(|&o| o != first && precedes(o, first)) {
            continue;
        }
        for &last in present {
            if (last == first) != (present.len() == 1)
                || present.iter().any(|&o| o != last && precedes(last, o))
            {
                continue;
            }
            let mut order = vec![first];
            let mut remaining: Vec<u32> = present
                .iter()
                .copied()
                .filter(|t| *t != first && *t != last)
                .collect();
            while let Some(index) = remaining.iter().position(|&candidate| {
                remaining
                    .iter()
                    .all(|&other| other == candidate || !precedes(other, candidate))
            }) {
                order.push(remaining.remove(index));
            }
            if remaining.is_empty() {
                if last != first {
                    order.push(last);
                }
                orders.push(order);
            }
        }
    }
    orders
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
        let orders = if context.order == ToolOrder::Free {
            candidate_orders(&present, |earlier, later| {
                before[b][earlier as usize * tools + later as usize]
            })
        } else {
            vec![present.clone()]
        };
        let mut orders = orders;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn permutations(tools: &[u32]) -> Vec<Vec<u32>> {
        if tools.len() <= 1 {
            return vec![tools.to_vec()];
        }
        let mut out = Vec::new();
        for i in 0..tools.len() {
            let mut rest = tools.to_vec();
            let head = rest.remove(i);
            for mut tail in permutations(&rest) {
                tail.insert(0, head);
                out.push(tail);
            }
        }
        out
    }

    /// For every (incoming tool, last tool) the construction picks exactly
    /// what exhaustive enumeration picks, and it still decides larger sets.
    #[test]
    fn pair_construction_matches_exhaustive_enumeration_without_a_tool_ceiling() {
        let mut random = 0x9E37_79B9_7F4A_7C15_u64;
        for trial in 0..400 {
            let n = 1 + trial % 6;
            let present: Vec<u32> = (0..n as u32).collect();
            let mut edges = vec![false; n * n];
            for a in 0..n {
                for b in a + 1..n {
                    random ^= random << 13;
                    random ^= random >> 7;
                    random ^= random << 17;
                    if random.is_multiple_of(4) {
                        // A random DAG over a random relabelling.
                        let (x, y) = if random.is_multiple_of(3) {
                            (b, a)
                        } else {
                            (a, b)
                        };
                        edges[x * n + y] = true;
                    }
                }
            }
            let precedes = |a: u32, b: u32| edges[a as usize * n + b as usize];
            let valid = |order: &Vec<u32>| {
                order
                    .iter()
                    .enumerate()
                    .all(|(i, later)| order[..i].iter().all(|earlier| !precedes(*later, *earlier)))
            };
            let all: Vec<_> = permutations(&present).into_iter().filter(valid).collect();
            let built = candidate_orders(&present, precedes);
            assert!(built.iter().all(valid));
            for incoming in [None, Some(0), Some(n as u32 - 1)] {
                let cost = |o: &Vec<u32>| o.len() as u32 - u32::from(incoming == Some(o[0]));
                for last in &present {
                    let best = |orders: &[Vec<u32>]| {
                        orders
                            .iter()
                            .filter(|o| o.last() == Some(last))
                            .map(|o| (cost(o), o.clone()))
                            .min()
                    };
                    assert_eq!(best(&built), best(&all), "{edges:?} {incoming:?} {last}");
                }
            }
        }
        let nine: Vec<u32> = (0..9).collect();
        let orders = candidate_orders(&nine, |a, b| a == 2 && b == 7);
        // Sources exclude 7, sinks exclude 2, and first differs from last.
        assert_eq!(orders.len(), 8 * 8 - 7);
    }
}
