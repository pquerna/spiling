// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Frozen independent judge. It shares no validation or pricing code with the
//! library: it re-derives admission and every metric from the case itself.
use super::cases::{Case, DEPOSITION_MM_S, distance};
use spiling_deposition_routing::*;

#[derive(Debug)]
pub struct Score {
    pub travel_mm: f64,
    pub starts: usize,
    pub deposition_mm: f64,
    pub selections: usize,
    /// Deposition plus straight travel at travel speed; no lift or retraction.
    pub motion_lower_bound_s: f64,
    pub machine_time_s: f64,
}

pub fn inspect(case: &Case, schedule: &Schedule) -> Result<Score, &'static str> {
    let n = case.strokes.len();
    let machine = &case.machine;
    if schedule.visits.len() != n {
        return Err("incomplete inventory");
    }
    let mut at = vec![usize::MAX; n];
    for (position, visit) in schedule.visits.iter().enumerate() {
        let index = visit.task as usize;
        let stroke = case.strokes.get(index).ok_or("unknown task")?;
        if at[index] != usize::MAX {
            return Err("duplicate task");
        }
        at[index] = position;
        if visit.reverse && (!stroke.reversible || stroke.free_seam) {
            return Err("locked direction");
        }
        if visit.seam != 0 && (!stroke.free_seam || visit.seam as usize >= stroke.points.len() - 1)
        {
            return Err("locked seam");
        }
    }
    for (index, stroke) in case.strokes.iter().enumerate() {
        if stroke
            .prerequisites
            .iter()
            .any(|q| at[*q as usize] >= at[index])
        {
            return Err("missing earlier prerequisite");
        }
    }
    if machine.change.service == Service::EveryBarrier {
        let mut required: Vec<_> = case.strokes.iter().map(|s| s.barrier).collect();
        required.sort_unstable();
        required.dedup();
        if schedule.groups.len() != required.len() * machine.tools as usize {
            return Err("a required service group is missing");
        }
    }
    let mut score = Score {
        travel_mm: 0.,
        starts: 0,
        deposition_mm: 0.,
        selections: 0,
        motion_lower_bound_s: 0.,
        machine_time_s: 0.,
    };
    let mut cursor = 0;
    let mut position = machine.start;
    let mut active = machine.initial_tool;
    let mut barrier = None;
    let mut tools_seen = vec![false; machine.tools as usize];
    let mut barriers = 0usize;
    for group in &schedule.groups {
        if group.visits.start != cursor || group.visits.end < cursor || group.visits.end > n {
            return Err("groups do not partition the visits");
        }
        cursor = group.visits.end;
        if group.tool >= machine.tools {
            return Err("unknown tool");
        }
        if barrier != Some(group.barrier) {
            if barrier.is_some_and(|b| b > group.barrier) {
                return Err("barrier out of order");
            }
            barrier = Some(group.barrier);
            barriers += 1;
            tools_seen.fill(false);
        }
        if std::mem::replace(&mut tools_seen[group.tool as usize], true) {
            return Err("tool split within a barrier");
        }
        if group.visits.is_empty() && machine.change.service == Service::WhenUsed {
            return Err("empty group without service");
        }
        let selected = active != Some(group.tool);
        let mut prewarm = None;
        if selected {
            if let Some(lead) = machine.change.prewarm_lead_s {
                let wait = (lead - score.machine_time_s).max(0.);
                score.machine_time_s += wait;
                prewarm = Some((score.machine_time_s - lead).max(0.));
            }
            score.selections += 1;
            score.machine_time_s += machine.change.swap_s;
            active = Some(group.tool);
        }
        score.machine_time_s += machine.change.service_s;
        let close = |a: f64, b: f64| {
            a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-9 * (1. + a.abs())
        };
        let same_prewarm = match (group.prewarm_at_s, prewarm) {
            (None, None) => true,
            (Some(a), Some(b)) => close(a, b),
            _ => false,
        };
        if group.selected != selected
            || !same_prewarm
            || !close(group.begins_at_s, score.machine_time_s)
        {
            return Err("tool transition clock annotation differs");
        }
        let station = machine.change.stations_xy[group.tool as usize];
        for (offset, visit) in schedule.visits[group.visits.clone()].iter().enumerate() {
            let stroke = &case.strokes[visit.task as usize];
            if stroke.tool != group.tool || stroke.barrier != group.barrier {
                return Err("task outside its tool group or barrier");
            }
            let (entry, exit) = if stroke.free_seam {
                (
                    stroke.points[visit.seam as usize],
                    stroke.points[visit.seam as usize],
                )
            } else if visit.reverse {
                (*stroke.points.last().unwrap(), stroke.points[0])
            } else {
                (stroke.points[0], *stroke.points.last().unwrap())
            };
            if offset == 0
                && let Some(xy) = station
            {
                position = Some([xy[0], xy[1], entry[2]]);
            }
            let straight = position.map_or(0., |from| distance(from, entry));
            if straight > 0. {
                let lifted = straight + 2. * machine.travel.lift_mm;
                score.travel_mm += lifted;
                score.machine_time_s += lifted / machine.travel.speed_mm_s;
                if let Some(r) = machine.travel.retraction
                    && straight >= r.minimum_mm
                {
                    score.machine_time_s += r.seconds + 2. * r.lift_mm / machine.travel.speed_mm_s;
                }
                score.motion_lower_bound_s += straight / machine.travel.speed_mm_s;
            }
            if straight > 0. || offset == 0 || position.is_none() {
                score.starts += 1;
            }
            let length = stroke.length();
            score.deposition_mm += length;
            score.machine_time_s += length / DEPOSITION_MM_S;
            score.motion_lower_bound_s += length / DEPOSITION_MM_S;
            position = Some(exit);
        }
        if group.visits.is_empty()
            && let (Some(xy), Some(p)) = (station, position)
        {
            position = Some([xy[0], xy[1], p[2]]);
        }
    }
    if cursor != n {
        return Err("groups do not cover the visits");
    }
    if machine.change.service == Service::EveryBarrier
        && schedule.groups.len() != barriers * machine.tools as usize
    {
        return Err("a required service group is missing");
    }
    if case.order == ToolOrder::Fixed
        && schedule
            .groups
            .windows(2)
            .any(|g| g[0].barrier == g[1].barrier && g[0].tool >= g[1].tool)
    {
        return Err("fixed tool order violated");
    }
    let reported = &schedule.cost;
    let close = |a: f64, b: f64| (a - b).abs() <= 1e-9 * (1. + a.abs());
    if !close(reported.travel_mm, score.travel_mm)
        || !close(reported.total_s, score.machine_time_s)
        || reported.starts as usize != score.starts
        || reported.selections as usize != score.selections
        || !score.machine_time_s.is_finite()
    {
        return Err("reported cost differs from independent evaluation");
    }
    Ok(score)
}

/// Sound relaxations for grading: no admitted schedule can score below either.
pub struct Bounds {
    pub starts: usize,
    pub travel_mm: f64,
}
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}
fn terminals(stroke: &super::cases::Stroke) -> Vec<PointMm> {
    if stroke.free_seam {
        stroke.points[..stroke.points.len() - 1].to_vec()
    } else {
        vec![stroke.points[0], *stroke.points.last().unwrap()]
    }
}
pub fn bounds(case: &Case) -> Bounds {
    // Every (barrier, tool) group is contiguous, so bound each one alone.
    let mut keys: Vec<(u32, u32)> = case.strokes.iter().map(|s| (s.barrier, s.tool)).collect();
    keys.sort_unstable();
    keys.dedup();
    let mut starts = 0;
    let mut travel_mm = 0.;
    for key in keys {
        let members: Vec<&super::cases::Stroke> = case
            .strokes
            .iter()
            .filter(|s| (s.barrier, s.tool) == key)
            .collect();
        // Starts: zero-travel chains are trails in the exact-endpoint graph.
        // Ignoring direction locks and prerequisites only shortens the cover.
        let mut ids = std::collections::HashMap::new();
        let mut parent = Vec::new();
        let mut degree = Vec::new();
        let mut vertex = |v: PointMm, parent: &mut Vec<usize>, degree: &mut Vec<usize>| {
            *ids.entry(v.map(|x| (x + 0.).to_bits())).or_insert_with(|| {
                parent.push(parent.len());
                degree.push(0usize);
                parent.len() - 1
            })
        };
        for s in &members {
            let ends = terminals(s);
            let first = vertex(ends[0], &mut parent, &mut degree);
            for v in &ends[1..] {
                let other = vertex(*v, &mut parent, &mut degree);
                let (a, b) = (find(&mut parent, first), find(&mut parent, other));
                parent[a] = b;
            }
            if !s.free_seam {
                let last = vertex(ends[1], &mut parent, &mut degree);
                degree[first] += 1;
                degree[last] += 1;
            }
        }
        let mut odd = vec![0usize; parent.len()];
        let mut used = vec![false; parent.len()];
        for (i, incident) in degree.iter().enumerate() {
            let root = find(&mut parent, i);
            used[root] = true;
            odd[root] += incident % 2;
        }
        starts += (0..parent.len())
            .filter(|i| used[*i])
            .map(|i| (odd[i] / 2).max(1))
            .sum::<usize>();
        // Travel: contracting each stroke, a group's travel spans its station
        // (when it has one) and all strokes: at least a minimum spanning tree.
        let mut nodes: Vec<Vec<PointMm>> = members.iter().map(|s| terminals(s)).collect();
        if let Some(xy) = case.machine.change.stations_xy[key.1 as usize] {
            nodes.push(vec![[xy[0], xy[1], members[0].points[0][2]]]);
        }
        let gap = |i: usize, j: usize| {
            nodes[i]
                .iter()
                .flat_map(|a| nodes[j].iter().map(|b| distance(*a, *b)))
                .fold(f64::INFINITY, f64::min)
        };
        let n = nodes.len();
        let mut best = vec![f64::INFINITY; n];
        let mut inside = vec![false; n];
        best[0] = 0.;
        for _ in 0..n {
            let i = (0..n)
                .filter(|i| !inside[*i])
                .min_by(|a, b| best[*a].total_cmp(&best[*b]))
                .unwrap();
            inside[i] = true;
            travel_mm += best[i];
            for j in 0..n {
                if !inside[j] {
                    best[j] = best[j].min(gap(i, j));
                }
            }
        }
    }
    Bounds { starts, travel_mm }
}
