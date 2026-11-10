// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Frozen recovery control, not an optimized or complete geometric solver.
use crate::{
    planner::{Session, State},
    *,
};
pub(crate) fn portals(session: &Session<'_>, state: &State) -> Result<Vec<PointMm>, Error> {
    let p = session.problem;
    let bottom = p.head_mm.iter().map(|b| b.min[2]).fold(0., f64::min);
    let mut result = Vec::new();
    for obstacle in &p.obstacles_mm {
        let z = obstacle.max[2] - bottom + 0.1;
        for v in std::iter::once(&state.position).chain(p.strokes.iter().flat_map(|s| &s.points_mm))
        {
            let v = PointMm([v.0[0], v.0[1], z]);
            if p.envelope_mm.contains(v) && !result.contains(&v) {
                if result.len() + p.waypoints_mm.len() >= session.limits.waypoints {
                    return Err(Error::Resource);
                }
                result.push(v);
            }
        }
    }
    Ok(result)
}
pub(crate) fn solve(session: &mut Session<'_>) -> Result<Route, Error> {
    let mut stack = vec![(session.empty(), None)];
    loop {
        session.charge(1)?;
        let Some((state, options)) = stack.last_mut() else {
            return Err(Error::NoRoute);
        };
        if state.steps().len() == session.problem.strokes.len() {
            return Ok(state.route());
        }
        if options.is_none() {
            let portals = portals(session, state)?;
            let mut choices = session.choices(state, &portals)?;
            choices.sort_by(|a, b| {
                a.step
                    .stroke
                    .cmp(&b.step.stroke)
                    .then(a.travel_mm.total_cmp(&b.travel_mm))
            });
            *options = Some(choices.into_iter());
        }
        if let Some(choice) = options.as_mut().unwrap().next() {
            session.charge((state.steps().len() + state.built().len()) as u64)?;
            let mut next = state.clone();
            session.push(&mut next, choice.step)?;
            if stack.len() >= session.limits.frontier_states {
                return Err(Error::Resource);
            }
            stack.push((next, None));
        } else {
            stack.pop();
        }
    }
}
