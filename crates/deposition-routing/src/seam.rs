// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Authored wall entry decisions in the scheduler. The production verifier
//! independently reconstructs them from emitted command coordinates.
//!
//! Seams follow the actual previous position through the whole order; no
//! seam history crosses walls or layers.
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

/// Sixteen build-XY directions around the compass, as exact small integers.
/// Only their order along a loop matters, not their length.
const COMPASS: [[f64; 2]; 16] = [
    [1., 0.],
    [2., 1.],
    [1., 1.],
    [1., 2.],
    [0., 1.],
    [-1., 2.],
    [-1., 1.],
    [-2., 1.],
    [-1., 0.],
    [-2., -1.],
    [-1., -1.],
    [-1., -2.],
    [0., -1.],
    [1., -2.],
    [1., -1.],
    [2., -1.],
];

/// The build-XY direction a wall's seam is pushed toward, or `None` when the
/// policy follows proximity alone. A Distributed wall faces compass entry
/// `(7 * layer + identity) mod 16`, where `identity` mixes the loop's least
/// projected vertex: seams turn 7/16 of a circle per layer and differ between
/// loops, without any history.
fn direction(task: &Task<'_>, seams: &[PointMm]) -> Result<Option<[f64; 2]>, Error> {
    Ok(match task.seam_policy {
        None | Some(SeamPolicy::Nearest) => None,
        Some(SeamPolicy::PreferredBuildDirection { xy }) => Some(xy),
        Some(SeamPolicy::Distributed) => {
            let mut least = project(seams[0])?;
            for raw in &seams[1..] {
                let p = project(*raw)?;
                if lex(p, least).is_lt() {
                    least = p;
                }
            }
            let key = |v: f64| (v * 1e6).round() as i64 as u64;
            let identity = (key(least[0]) ^ key(least[1]).rotate_left(32))
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                >> 60;
            Some(COMPASS[((7 * u64::from(task.layer) + identity) % 16) as usize])
        }
    })
}

fn score(direction: Option<[f64; 2]>, p: PointMm) -> f64 {
    direction.map_or(0.0, |xy| zero(p[0] * xy[0] + p[1] * xy[1]))
}

/// Author every policy wall's seam: highest policy score, then nearest the
/// actual previous position (the previous path's end, across layers), then
/// projected XY, raw XY and index. One cheap serial pass over the order.
pub(crate) fn apply(
    tasks: &[Task<'_>],
    machine: &Machine,
    visits: &mut [Visit],
    groups: &[(u32, u32, Range<usize>)],
    cancel: Option<&AtomicBool>,
) -> Result<(), Error> {
    let mut position = machine.start;
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
            if let (Some(_), Ends::Loop { seams }) = (task.seam_policy, task.ends) {
                let direction = direction(task, seams)?;
                let mut best = 0usize;
                let mut point = project(seams[0])?;
                for (index, raw) in seams.iter().enumerate().skip(1) {
                    if let Some(cancel) = cancel {
                        cancelled(cancel)?;
                    }
                    let candidate = project(*raw)?;
                    let ordering = score(direction, point)
                        .total_cmp(&score(direction, candidate))
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
                position = Some(point);
            } else {
                position = Some(project(entry_exit(task, *visit).1)?);
            }
        }
    }
    Ok(())
}

/// The seams a directional wall (preferred build direction or distributed)
/// can receive: every vertex tied at the policy's best score, ascending.
/// [`apply`] chooses among exactly these by proximity, so the search need
/// rank no other entry. `None` for other tasks, whose entry depends on the
/// order, and for coordinates [`apply`] itself refuses.
pub(crate) fn policy_seams(task: &Task<'_>) -> Option<Box<[u32]>> {
    let Ends::Loop { seams } = task.ends else {
        return None;
    };
    let direction = direction(task, seams).ok()??;
    let mut best = f64::NEG_INFINITY;
    let mut tied = Vec::new();
    for (index, raw) in seams.iter().enumerate() {
        let score = score(Some(direction), project(*raw).ok()?);
        match score.total_cmp(&best) {
            std::cmp::Ordering::Greater => {
                best = score;
                tied.clear();
                tied.push(index as u32);
            }
            std::cmp::Ordering::Equal => tied.push(index as u32),
            std::cmp::Ordering::Less => {}
        }
    }
    Some(tied.into_boxed_slice())
}
