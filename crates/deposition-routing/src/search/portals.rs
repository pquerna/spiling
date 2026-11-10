// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Local-height motion primitives; every visibility edge is checked downstream.
use crate::{
    planner::{Session, State},
    *,
};
pub(super) fn propose(session: &mut Session<'_>, state: &State) -> Result<Vec<PointMm>, Error> {
    let p = session.problem;
    let bottom = p.head_mm.iter().map(|b| b.min[2]).fold(0., f64::min);
    let mut points = Vec::new();
    let mut heights = Vec::new();
    for obstacle in p.obstacles_mm.iter().chain(state.built()) {
        session.charge(1)?;
        heights.push(obstacle.max[2] - bottom + 0.1);
    }
    heights.sort_by(f64::total_cmp);
    heights.dedup();
    for z in heights {
        for v in std::iter::once(&state.position).chain(p.strokes.iter().flat_map(|s| &s.points_mm))
        {
            session.charge(1 + points.len() as u64)?;
            let portal = PointMm([v.0[0], v.0[1], z]);
            if p.envelope_mm.contains(portal) && !points.contains(&portal) {
                if points.len() + p.waypoints_mm.len() >= session.limits.waypoints {
                    return Err(Error::Resource);
                }
                points.push(portal);
            }
        }
    }
    Ok(points)
}
