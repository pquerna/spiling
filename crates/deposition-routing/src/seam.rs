// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Authored wall entry decisions in the scheduler. The production verifier
//! independently reconstructs them from emitted command coordinates.
use crate::*;
use spiling_contracts::manufacturing::program_coordinates::project_xyz6_point_mm;

fn project(p: PointMm) -> Result<PointMm, Error> {
    project_xyz6_point_mm(p).map_err(|_| Error::InvalidInput("wall command coordinate"))
}
fn zero(v: f64) -> f64 {
    if v == 0.0 { 0.0 } else { v }
}
fn lex(a: PointMm, b: PointMm) -> std::cmp::Ordering {
    zero(a[0])
        .total_cmp(&zero(b[0]))
        .then_with(|| zero(a[1]).total_cmp(&zero(b[1])))
}
fn distance(a: PointMm, b: PointMm) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}
pub(crate) fn apply(
    tasks: &[Task<'_>],
    machine: &Machine,
    visits: &mut [Visit],
    groups: &[(u32, u32, Range<usize>)],
    cancel: Option<&AtomicBool>,
) -> Result<(), Error> {
    let mut position = machine.start;
    let mut history = vec![[[0.0; 2]; 8]; machine.tools as usize];
    let mut counts = vec![0usize; machine.tools as usize];
    let mut cursors = vec![0usize; machine.tools as usize];
    for (_, tool, range) in groups {
        let first = visits
            .get(range.start)
            .copied()
            .filter(|_| !range.is_empty());
        position = cost::group_start(tasks, machine, *tool, first, position);
        if first
            .is_some_and(|v| tasks[v.task as usize].seam_policy == Some(SeamPolicy::Distributed))
        {
            position = position.map(project).transpose()?;
        }
        for visit in &mut visits[range.clone()] {
            if let Some(cancel) = cancel {
                cancelled(cancel)?;
            }
            let task = &tasks[visit.task as usize];
            if let (Some(policy), Ends::Loop { seams }) = (task.seam_policy, task.ends) {
                let recent = &history[*tool as usize][..counts[*tool as usize]];
                let score = |p: PointMm| match policy {
                    SeamPolicy::Nearest => 0.0,
                    SeamPolicy::Distributed => recent
                        .iter()
                        .map(|h| (p[0] - h[0]).hypot(p[1] - h[1]))
                        .fold(f64::INFINITY, f64::min),
                    SeamPolicy::PreferredBuildDirection { xy } => zero(p[0] * xy[0] + p[1] * xy[1]),
                };
                let mut best = 0usize;
                let mut point = project(seams[0])?;
                for (index, raw) in seams.iter().enumerate().skip(1) {
                    if let Some(cancel) = cancel {
                        cancelled(cancel)?;
                    }
                    let candidate = project(*raw)?;
                    let ordering = score(point)
                        .total_cmp(&score(candidate))
                        .then_with(|| {
                            position.map_or(std::cmp::Ordering::Equal, |from| {
                                distance(from, candidate).total_cmp(&distance(from, point))
                            })
                        })
                        .then_with(|| lex(candidate, point))
                        .then_with(|| lex(*raw, seams[best]))
                        .then_with(|| index.cmp(&best));
                    if ordering.is_lt() {
                        best = index;
                        point = candidate;
                    }
                }
                visit.seam = best as u32;
                if policy == SeamPolicy::Distributed {
                    let t = *tool as usize;
                    history[t][cursors[t]] = [point[0], point[1]];
                    cursors[t] = (cursors[t] + 1) % 8;
                    counts[t] = (counts[t] + 1).min(8);
                }
                position = Some(point);
            } else {
                position = Some(project(entry_exit(task, *visit).1)?);
            }
        }
    }
    Ok(())
}
