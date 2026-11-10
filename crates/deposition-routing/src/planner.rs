// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use crate::*;

pub(crate) struct Choice {
    pub step: Step,
    pub travel_mm: f64,
    pub following_distance_mm: f64,
}
fn valid_box(b: BoundsMm) -> bool {
    (0..3).all(|a| {
        b.min[a].is_finite()
            && b.max[a].is_finite()
            && b.min[a] < b.max[a]
            && (b.max[a] - b.min[a]).is_finite()
    })
}
fn validate(p: &Problem, limits: Limits, meter: &mut Meter<'_>) -> Result<(), Error> {
    meter.charge(1)?;
    let edges = p
        .strokes
        .iter()
        .try_fold(0usize, |n, s| n.checked_add(s.widths_mm.len()))
        .ok_or(Error::Resource)?;
    if edges > limits.edges || p.waypoints_mm.len() > limits.waypoints {
        return Err(Error::Resource);
    }
    if p.strokes.is_empty()
        || p.head_mm.is_empty()
        || !valid_box(p.envelope_mm)
        || !PointMm(p.envelope_mm.min)
            .distance(PointMm(p.envelope_mm.max))
            .is_finite()
        || !p.envelope_mm.contains(p.start_mm)
        || p.head_mm
            .iter()
            .chain(&p.obstacles_mm)
            .any(|b| !valid_box(*b))
        || [p.travel_speed_mm_s, p.deposition_speed_mm_s]
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.)
        || p.waypoints_mm.iter().any(|v| !p.envelope_mm.contains(*v))
    {
        return Err(Error::InvalidInput);
    }
    for (i, s) in p.strokes.iter().enumerate() {
        meter.charge((p.strokes.len() + s.prerequisites.len()) as u64)?;
        let h = s.physical_z_mm[1] - s.physical_z_mm[0];
        if s.points_mm.len() < 2
            || s.widths_mm.len() != s.points_mm.len() - 1
            || !h.is_finite()
            || h <= 0.
            || !s.physical_z_mm[0].is_finite()
            || !s.physical_z_mm[1].is_finite()
            || s.points_mm
                .iter()
                .any(|pnt| !p.envelope_mm.contains(*pnt) || pnt.0[2] != s.physical_z_mm[1])
            || s.points_mm
                .windows(2)
                .any(|p| p[0] == p[1] || !p[0].distance(p[1]).is_finite())
            || s.widths_mm.iter().any(|w| !w.is_finite() || *w <= h)
            || p.strokes[..i].iter().any(|other| other.id == s.id)
            || s.prerequisites
                .iter()
                .any(|id| *id == s.id || !p.strokes.iter().any(|x| x.id == *id))
            || (s.allow_seam_rotation && s.points_mm.first() != s.points_mm.last())
            || bead_boxes(s).any(|b| !valid_box(b))
        {
            return Err(Error::InvalidInput);
        }
    }
    // Dependency cycles are malformed input, not an obstructed scene.
    let mut done = vec![false; p.strokes.len()];
    for _ in &p.strokes {
        let mut progress = false;
        for (i, s) in p.strokes.iter().enumerate() {
            meter.charge(1)?;
            if !done[i]
                && s.prerequisites.iter().all(|id| {
                    p.strokes
                        .iter()
                        .position(|x| x.id == *id)
                        .is_some_and(|j| done[j])
                })
            {
                done[i] = true;
                progress = true;
            }
        }
        if done.iter().all(|v| *v) {
            return Ok(());
        }
        if !progress {
            return Err(Error::InvalidInput);
        }
    }
    Err(Error::InvalidInput)
}
pub fn plan(p: &Problem, limits: Limits, cancel: &AtomicBool) -> Result<Route, Error> {
    construct(p, limits, cancel, search::choose)
}
pub fn plan_input_order(p: &Problem, limits: Limits, cancel: &AtomicBool) -> Result<Route, Error> {
    construct(p, limits, cancel, |choices| {
        (0..choices.len())
            .min_by(|a, b| {
                choices[*a]
                    .step
                    .stroke
                    .cmp(&choices[*b].step.stroke)
                    .then(choices[*a].travel_mm.total_cmp(&choices[*b].travel_mm))
            })
            .unwrap()
    })
}
fn construct(
    p: &Problem,
    limits: Limits,
    cancel: &AtomicBool,
    select: impl Fn(&[Choice]) -> usize,
) -> Result<Route, Error> {
    let mut meter = Meter {
        used: 0,
        limit: limits.work,
        cancel,
    };
    validate(p, limits, &mut meter)?;
    let mut route = Route {
        steps: Vec::new(),
        work: 0,
    };
    let mut built = Vec::new();
    let mut done = vec![false; p.strokes.len()];
    let mut position = p.start_mm;
    while route.steps.len() < p.strokes.len() {
        let mut choices = Vec::new();
        for (i, s) in p.strokes.iter().enumerate() {
            meter.charge(1)?;
            if done[i]
                || !s.prerequisites.iter().all(|id| {
                    p.strokes
                        .iter()
                        .position(|x| x.id == *id)
                        .is_some_and(|j| done[j])
                })
            {
                continue;
            }
            for reverse in [false, true]
                .into_iter()
                .take(if s.allow_reverse { 2 } else { 1 })
            {
                for seam in 0..if s.allow_seam_rotation {
                    s.widths_mm.len()
                } else {
                    1
                } {
                    let edges = traversal(s, reverse, seam);
                    let mut clear = true;
                    for &(a, b, _) in &edges {
                        if !clearance::clear(p, &built, a, b, &mut meter)? {
                            clear = false;
                            break;
                        }
                    }
                    if !clear {
                        continue;
                    }
                    let Some(arrival) = travel::path(p, &built, position, edges[0].0, &mut meter)?
                    else {
                        continue;
                    };
                    let previous = route.steps.last();
                    let continuous = previous.is_some_and(|last| {
                        let old = &p.strokes[last.stroke];
                        let e = traversal(old, last.reverse, last.seam);
                        let tail = e.last().unwrap();
                        old.process == s.process
                            && old.physical_z_mm == s.physical_z_mm
                            && tail.1 == edges[0].0
                            && tail.2 == edges[0].2
                    });
                    let end = edges.last().unwrap().1;
                    let mut following_distance_mm = f64::INFINITY;
                    for (j, next) in p.strokes.iter().enumerate() {
                        meter.charge(1)?;
                        if !done[j] && j != i {
                            following_distance_mm =
                                following_distance_mm.min(end.distance(next.points_mm[0]));
                            if next.allow_reverse {
                                following_distance_mm = following_distance_mm
                                    .min(end.distance(*next.points_mm.last().unwrap()));
                            }
                        }
                    }
                    if !following_distance_mm.is_finite() {
                        following_distance_mm = 0.;
                    }
                    choices.push(Choice {
                        travel_mm: arrival.windows(2).map(|p| p[0].distance(p[1])).sum(),
                        following_distance_mm,
                        step: Step {
                            stroke: i,
                            reverse,
                            seam,
                            travel_mm: arrival,
                            continuous,
                        },
                    });
                }
            }
        }
        if choices.is_empty() {
            return Err(Error::NoRoute);
        }
        let chosen = select(&choices);
        if chosen >= choices.len() {
            return Err(Error::InvalidInput);
        }
        let step = choices.swap_remove(chosen).step;
        let stroke = &p.strokes[step.stroke];
        position = traversal(stroke, step.reverse, step.seam).last().unwrap().1;
        done[step.stroke] = true;
        built.extend(bead_boxes(stroke));
        route.steps.push(step);
    }
    route.work = meter.used;
    Ok(route)
}
