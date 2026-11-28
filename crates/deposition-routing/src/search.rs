// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Editable scheduling algorithm. Its output is admitted only by the frozen
//! checker; nothing here can add, drop or alter a task.
mod construct;
mod improve;
mod sequence;
use crate::*;
use rayon::prelude::*;

pub(crate) struct Context<'a, 't> {
    pub tasks: &'a [Task<'t>],
    pub machine: &'a Machine,
    pub order: ToolOrder,
    pub cancel: &'a AtomicBool,
    /// Counted search work: reported and checked for cancellation, never a limit.
    pub work: u64,
}
impl Context<'_, '_> {
    /// Count work; cancellation is the only way the search stops early.
    pub fn charge(&mut self, amount: u64) -> Result<(), Error> {
        cancelled(self.cancel)?;
        self.work = self.work.saturating_add(amount);
        Ok(())
    }
}

/// Search-local gap pricing: the frozen travel model with a plain square root
/// instead of the frozen `hypot`. It only ranks candidate orders; the frozen
/// cost re-prices whatever order the search returns.
#[derive(Clone, Copy)]
pub(crate) struct Pricing {
    lift: f64,
    speed: f64,
    minimum: f64,
    retraction: f64,
}
impl Pricing {
    pub fn new(travel: &Travel) -> Self {
        let (minimum, retraction) = travel.retraction.map_or((f64::INFINITY, 0.), |r| {
            (r.minimum_mm, r.seconds + 2. * r.lift_mm / travel.speed_mm_s)
        });
        Self {
            lift: 2. * travel.lift_mm,
            speed: travel.speed_mm_s,
            minimum,
            retraction,
        }
    }
    #[inline(always)]
    pub fn seconds(&self, a: PointMm, b: PointMm) -> f64 {
        let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
        let d = (dx * dx + dy * dy + dz * dz).sqrt();
        // Branch-free selects so slice passes vectorize.
        let extra = if d >= self.minimum {
            self.retraction
        } else {
            0.
        };
        let seconds = (d + self.lift) / self.speed + extra;
        if d == 0. { 0. } else { seconds }
    }
}

pub(crate) type Groups = Vec<(u32, u32, Range<usize>)>;
/// One layer's visits, layer-local groups and the search's end position.
type RoutedLayer = (Vec<Visit>, Groups, Option<PointMm>);

/// Tasks bucketed by barrier (ascending), then by tool.
struct Buckets {
    barriers: Vec<u32>,
    /// The layer of each barrier (validated: one per barrier, ascending).
    layers: Vec<u32>,
    /// `members[barrier_index][tool]` holds task indexes in input order.
    members: Vec<Vec<Vec<u32>>>,
}
fn buckets(context: &Context<'_, '_>) -> Buckets {
    let mut barriers: Vec<u32> = context.tasks.iter().map(|t| t.barrier).collect();
    barriers.sort_unstable();
    barriers.dedup();
    let tools = context.machine.tools as usize;
    let mut members = vec![vec![Vec::new(); tools]; barriers.len()];
    let mut layers = vec![0; barriers.len()];
    for (index, task) in context.tasks.iter().enumerate() {
        let b = barriers.binary_search(&task.barrier).unwrap();
        members[b][task.tool as usize].push(index as u32);
        layers[b] = task.layer;
    }
    Buckets {
        barriers,
        layers,
        members,
    }
}
fn present(context: &Context<'_, '_>, tools: &[Vec<u32>]) -> Vec<u32> {
    (0..context.machine.tools)
        .filter(|t| {
            context.machine.change.service == Service::EveryBarrier
                || !tools[*t as usize].is_empty()
        })
        .collect()
}

/// Route every layer in parallel from two starts, then choose per layer.
///
/// Pass 1 routes each layer from the machine start. Pass 2 routes each layer
/// again from pass 1's end of the layer below. A serial pass over the layers
/// then keeps, per layer, whichever candidate travels less from the end the
/// chosen layers below actually reached (ties to pass 1). Both passes are
/// parallel and independent of the choice, so the result is deterministic;
/// a long part alternates its start side instead of returning each layer.
pub(crate) fn solve(context: &mut Context<'_, '_>) -> Result<(Vec<Visit>, Groups), Error> {
    let avx2 = std::env::var_os("SPILING_ROUTING_AVX2").is_none_or(|value| value == "1");
    let buckets = buckets(context);
    let orders = sequence::plan(context, &buckets.barriers, &buckets.members)?;
    // Contiguous barrier-index runs of one layer.
    let mut layers: Vec<Range<usize>> = Vec::new();
    for b in 0..buckets.barriers.len() {
        match layers.last_mut() {
            Some(run) if buckets.layers[run.start] == buckets.layers[b] => run.end = b + 1,
            _ => layers.push(b..b + 1),
        }
    }
    let (tasks, machine, order, cancel) = (
        context.tasks,
        context.machine,
        context.order,
        context.cancel,
    );
    let route = |barriers: Range<usize>, start: Option<PointMm>| {
        let mut layer = Context {
            tasks,
            machine,
            order,
            cancel,
            work: 0,
        };
        let routed = route_layer(&mut layer, &buckets, &orders, barriers, start, avx2)?;
        Ok((routed, layer.work))
    };
    let first: Vec<Result<(RoutedLayer, u64), Error>> = layers
        .par_iter()
        .map(|barriers| route(barriers.clone(), machine.start))
        .collect();
    let first = first.into_iter().collect::<Result<Vec<_>, Error>>()?;
    let second: Vec<Result<Option<(RoutedLayer, u64)>, Error>> = (0..layers.len())
        .into_par_iter()
        .map(
            |l| match l.checked_sub(1).and_then(|below| first[below].0.2) {
                Some(start)
                    if Some(start) != machine.start
                        && layers[l].clone().any(|b| {
                            orders[b].iter().any(|&tool| {
                                !buckets.members[b][tool as usize].is_empty()
                                    && machine.change.stations_xy[tool as usize].is_none()
                            })
                        }) =>
                {
                    route(layers[l].clone(), Some(start)).map(Some)
                }
                _ => Ok(None),
            },
        )
        .collect();
    let second = second.into_iter().collect::<Result<Vec<_>, Error>>()?;
    let total: usize = orders.iter().map(Vec::len).sum();
    let mut visits = Vec::with_capacity(context.tasks.len());
    let mut groups = Vec::with_capacity(total);
    let mut position = machine.start;
    for (pass1, pass2) in first.into_iter().zip(second) {
        context.work = context.work.saturating_add(pass1.1);
        let mut chosen = pass1.0;
        if let Some((candidate, work)) = pass2 {
            context.work = context.work.saturating_add(work);
            let (keep, _) = travel_from(tasks, machine, &chosen, position);
            let (other, _) = travel_from(tasks, machine, &candidate, position);
            if other < keep {
                chosen = candidate;
            }
        }
        position = travel_from(tasks, machine, &chosen, position).1;
        let (layer_visits, layer_groups, _) = chosen;
        let base = visits.len();
        visits.extend(layer_visits);
        groups.extend(
            layer_groups.into_iter().map(|(barrier, tool, range)| {
                (barrier, tool, range.start + base..range.end + base)
            }),
        );
    }
    Ok((visits, groups))
}

/// Modelled travel seconds of one routed layer entered from `from`, and
/// where it ends.
fn travel_from(
    tasks: &[Task<'_>],
    machine: &Machine,
    (visits, groups, _): &RoutedLayer,
    from: Option<PointMm>,
) -> (f64, Option<PointMm>) {
    let mut seconds = 0.;
    let mut position = from;
    for (_, tool, range) in groups {
        let first = visits
            .get(range.start)
            .copied()
            .filter(|_| !range.is_empty());
        position = cost::group_start(tasks, machine, *tool, first, position);
        for visit in &visits[range.clone()] {
            let (entry, exit) = entry_exit(&tasks[visit.task as usize], *visit);
            if let Some(p) = position {
                seconds += gap_seconds(&machine.travel, p, entry);
            }
            position = Some(exit);
        }
    }
    (seconds, position)
}

/// One layer's groups from `start`: layer-local ranges and the search's end.
fn route_layer(
    context: &mut Context<'_, '_>,
    buckets: &Buckets,
    orders: &[Vec<u32>],
    barriers: Range<usize>,
    start: Option<PointMm>,
    avx2: bool,
) -> Result<RoutedLayer, Error> {
    let mut visits = Vec::new();
    let mut groups = Vec::new();
    let mut position = start;
    for b in barriers {
        for &tool in &orders[b] {
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
                let mut order = construct::nearest(context, members, start)?;
                if avx2 {
                    order.enable_simd(context.cancel);
                }
                improve::improve(context, &mut order)?;
                position = order.end().or(start);
                visits.extend(order.into_visits(members));
            } else if let (Some(xy), Some(p)) =
                (context.machine.change.stations_xy[tool as usize], position)
            {
                position = Some([xy[0], xy[1], p[2]]);
            }
            groups.push((buckets.barriers[b], tool, begin..visits.len()));
        }
    }
    Ok((visits, groups, position))
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
