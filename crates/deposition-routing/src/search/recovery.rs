// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Bounded iterative backtracking: an inexpensive dead end need not end search.
//! Direct and high-plane travel are tried first; local-height portals are only
//! proposed for a prefix whose cheaper choices are exhausted.
use crate::{planner::Session, *};
pub(super) fn solve(session: &mut Session<'_>) -> Result<Route, Error> {
    let mut stack = vec![(session.empty(), None, false)];
    loop {
        session.charge(1)?;
        let Some((state, options, widened)) = stack.last_mut() else {
            return Err(Error::NoRoute);
        };
        if state.steps().len() == session.problem.strokes.len() {
            return Ok(state.route());
        }
        if options.is_none() {
            let portals = if *widened {
                super::portals::propose(session, state)?
            } else {
                vec![]
            };
            let mut choices = session.choices(state, &portals)?;
            choices.sort_by(|a, b| {
                a.travel_mm
                    .total_cmp(&b.travel_mm)
                    .then(a.following_distance_mm.total_cmp(&b.following_distance_mm))
            });
            *options = Some(choices.into_iter());
        }
        if let Some(choice) = options.as_mut().unwrap().next() {
            session.charge(
                (session.problem.strokes.len() + state.steps().len() + state.built().len()) as u64,
            )?;
            let mut next = state.clone();
            session.push(&mut next, choice.step)?;
            if stack.len() >= session.limits.frontier_states {
                return Err(Error::Resource);
            }
            stack.push((next, None, false));
        } else if *widened {
            stack.pop();
        } else {
            *widened = true;
            *options = None;
        }
    }
}
