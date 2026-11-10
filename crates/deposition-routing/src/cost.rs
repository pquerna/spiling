// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Frozen modelled cost and schedule clock. Declared values, not measurements.
use crate::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cost {
    pub travel_mm: f64,
    pub travel_s: f64,
    /// Non-zero travel moves, plus the first entry of every non-empty group.
    pub starts: u32,
    pub selections: u32,
    pub change_s: f64,
    /// Unmet declared prewarm lead at the start of the modeled timeline.
    pub warmup_s: f64,
    pub deposition_s: f64,
    pub total_s: f64,
}

/// Straight tip distance, plus the declared lift up and down when moving.
pub(crate) fn gap_mm(travel: &Travel, a: PointMm, b: PointMm) -> f64 {
    let d = (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2]);
    if d > 0. { d + 2. * travel.lift_mm } else { 0. }
}

/// Modelled seconds to travel from one exit to the next entry.
pub fn gap_seconds(travel: &Travel, a: PointMm, b: PointMm) -> f64 {
    let d = (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2]);
    if d == 0. {
        return 0.;
    }
    let mut seconds = (d + 2. * travel.lift_mm) / travel.speed_mm_s;
    if let Some(r) = travel.retraction
        && d >= r.minimum_mm
    {
        seconds += r.seconds + 2. * r.lift_mm / travel.speed_mm_s;
    }
    seconds
}

/// Where a group's motion begins: its tool's station at the group's height,
/// otherwise wherever the previous motion ended.
pub(crate) fn group_start(
    tasks: &[Task<'_>],
    machine: &Machine,
    tool: u32,
    first: Option<Visit>,
    previous: Option<PointMm>,
) -> Option<PointMm> {
    match (machine.change.stations_xy[tool as usize], first) {
        (Some(xy), Some(visit)) => {
            let z = entry_exit(&tasks[visit.task as usize], visit).0[2];
            Some([xy[0], xy[1], z])
        }
        (Some(xy), None) => previous.map(|p| [xy[0], xy[1], p[2]]),
        (None, _) => previous,
    }
}

/// Price a visit order and place tool selections and pre-warm on one clock.
pub(crate) fn annotate(
    tasks: &[Task<'_>],
    machine: &Machine,
    visits: Vec<Visit>,
    groups: Vec<(u32, u32, Range<usize>)>,
) -> Schedule {
    let mut cost = Cost::default();
    let mut clock = 0.;
    let mut position = machine.start;
    let mut active = machine.initial_tool;
    let mut annotated = Vec::with_capacity(groups.len());
    for (barrier, tool, range) in groups {
        let selected = active != Some(tool);
        let mut prewarm_at_s = None;
        if selected {
            prewarm_at_s = machine.change.prewarm_lead_s.map(|lead| {
                let wait = (lead - clock).max(0.);
                cost.warmup_s += wait;
                clock += wait;
                (clock - lead).max(0.)
            });
            cost.selections += 1;
            cost.change_s += machine.change.swap_s;
            clock += machine.change.swap_s;
            active = Some(tool);
        }
        cost.change_s += machine.change.service_s;
        clock += machine.change.service_s;
        let begins_at_s = clock;
        position = group_start(
            tasks,
            machine,
            tool,
            visits
                .get(range.start)
                .copied()
                .filter(|_| !range.is_empty()),
            position,
        );
        for (offset, visit) in visits[range.clone()].iter().enumerate() {
            let task = &tasks[visit.task as usize];
            let (entry, exit) = entry_exit(task, *visit);
            if let Some(from) = position {
                let mm = gap_mm(&machine.travel, from, entry);
                let seconds = gap_seconds(&machine.travel, from, entry);
                cost.travel_mm += mm;
                cost.travel_s += seconds;
                clock += seconds;
                cost.starts += u32::from(mm > 0. || offset == 0);
            } else {
                cost.starts += 1;
            }
            cost.deposition_s += task.duration_s;
            clock += task.duration_s;
            position = Some(exit);
        }
        annotated.push(Group {
            barrier,
            tool,
            visits: range,
            selected,
            prewarm_at_s,
            begins_at_s,
        });
    }
    cost.total_s = cost.travel_s + cost.change_s + cost.warmup_s + cost.deposition_s;
    Schedule {
        visits,
        groups: annotated,
        cost,
        work: 0,
    }
}
