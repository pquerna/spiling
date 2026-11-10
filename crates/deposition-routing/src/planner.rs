// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use crate::*;

#[derive(Clone)]
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
/// A checked route prefix; clones share immutable Problem geometry.
#[derive(Clone)]
pub(crate) struct State {
    steps: Vec<Step>,
    built: Vec<BoundsMm>,
    done: Vec<bool>,
    pub position: PointMm,
    pub travel_mm: f64,
    pub starts: usize,
}
impl State {
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }
    pub fn done(&self, index: usize) -> bool {
        self.done[index]
    }
    pub fn built(&self) -> &[BoundsMm] {
        &self.built
    }
    pub fn route(&self) -> Route {
        Route {
            steps: self.steps.clone(),
            work: 0,
        }
    }
}
/// Frozen checked primitives, not an ordering policy. Candidates may also
/// construct arbitrary Steps/Routes; final replay and independent judging apply.
pub(crate) struct Session<'a> {
    pub problem: &'a Problem,
    pub limits: Limits,
    meter: Meter<'a>,
}
impl<'a> Session<'a> {
    fn new(p: &'a Problem, limits: Limits, cancel: &'a AtomicBool) -> Self {
        Self {
            problem: p,
            limits,
            meter: Meter {
                used: 0,
                limit: limits.work,
                cancel,
            },
        }
    }
    pub fn charge(&mut self, work: u64) -> Result<(), Error> {
        self.meter.charge(work)
    }
    #[allow(dead_code)] // Available to algorithm trials that keep an incumbent.
    pub fn remaining_work(&self) -> u64 {
        self.limits.work.saturating_sub(self.meter.used)
    }
    pub fn empty(&self) -> State {
        State {
            steps: vec![],
            built: vec![],
            done: vec![false; self.problem.strokes.len()],
            position: self.problem.start_mm,
            travel_mm: 0.,
            starts: 0,
        }
    }
    pub fn clear(&mut self, state: &State, a: PointMm, b: PointMm) -> Result<bool, Error> {
        self.charge(1)?;
        clearance::clear(self.problem, &state.built, a, b, &mut self.meter)
    }
    pub fn travel(
        &mut self,
        state: &State,
        to: PointMm,
        portals: &[PointMm],
    ) -> Result<Option<Vec<PointMm>>, Error> {
        if portals
            .len()
            .checked_add(self.problem.waypoints_mm.len())
            .is_none_or(|n| n > self.limits.waypoints)
        {
            return Err(Error::Resource);
        }
        if portals
            .iter()
            .any(|p| !self.problem.envelope_mm.contains(*p))
        {
            return Err(Error::InvalidInput);
        }
        travel::path(
            self.problem,
            &state.built,
            state.position,
            to,
            portals,
            &mut self.meter,
        )
    }
    fn ready(&self, state: &State, index: usize) -> bool {
        !state.done(index)
            && self.problem.strokes[index].prerequisites.iter().all(|id| {
                self.problem
                    .strokes
                    .iter()
                    .position(|s| s.id == *id)
                    .is_some_and(|j| state.done[j])
            })
    }
    fn continuous(&self, state: &State, step: &Step) -> bool {
        state.steps.last().is_some_and(|last| {
            let old = &self.problem.strokes[last.stroke];
            let new = &self.problem.strokes[step.stroke];
            let a = traversal(old, last.reverse, last.seam);
            let b = traversal(new, step.reverse, step.seam);
            old.process == new.process
                && old.physical_z_mm == new.physical_z_mm
                && a.last().unwrap().1 == b[0].0
                && a.last().unwrap().2 == b[0].2
        })
    }
    fn deposition_clear(
        &mut self,
        state: &State,
        source: &Stroke,
        edges: &[(PointMm, PointMm, f64)],
    ) -> Result<bool, Error> {
        let mut earlier = Vec::new();
        for &(a, b, width) in edges {
            if !self.clear(state, a, b)?
                || !clearance::clear(self.problem, &earlier, a, b, &mut self.meter)?
            {
                return Ok(false);
            }
            earlier.push(bead_bounds(a, b, width, source.physical_z_mm));
        }
        Ok(true)
    }
    /// Custom travel and ordering are accepted only after checking the entire step.
    pub fn push(&mut self, state: &mut State, step: Step) -> Result<(), Error> {
        self.charge(1)?;
        let p = self.problem;
        if step.stroke >= p.strokes.len() || !self.ready(state, step.stroke) {
            return Err(Error::InvalidInput);
        }
        let s = &p.strokes[step.stroke];
        if (step.reverse && !s.allow_reverse)
            || step.seam >= s.widths_mm.len()
            || (!s.allow_seam_rotation && step.seam != 0)
        {
            return Err(Error::InvalidInput);
        }
        self.charge(s.widths_mm.len() as u64)?;
        let edges = traversal(s, step.reverse, step.seam);
        if step.travel_mm.is_empty()
            || step.travel_mm.len() > self.limits.waypoints.saturating_add(4)
            || step.travel_mm[0] != state.position
            || *step.travel_mm.last().unwrap() != edges[0].0
            || (step.continuous && (!self.continuous(state, &step) || step.travel_mm.len() != 1))
        {
            return Err(Error::InvalidInput);
        }
        for pair in step.travel_mm.windows(2) {
            if !self.clear(state, pair[0], pair[1])? {
                return Err(Error::InvalidInput);
            }
        }
        if !self.deposition_clear(state, s, &edges)? {
            return Err(Error::InvalidInput);
        }
        state.travel_mm += step
            .travel_mm
            .windows(2)
            .map(|p| p[0].distance(p[1]))
            .sum::<f64>();
        if !state.travel_mm.is_finite() {
            return Err(Error::InvalidInput);
        }
        state.starts += usize::from(!step.continuous);
        state.position = edges.last().unwrap().1;
        state.done[step.stroke] = true;
        state.built.extend(bead_boxes(s));
        state.steps.push(step);
        Ok(())
    }
    pub fn choices(&mut self, state: &State, portals: &[PointMm]) -> Result<Vec<Choice>, Error> {
        let p = self.problem;
        let mut choices = Vec::new();
        for (i, s) in p.strokes.iter().enumerate() {
            self.charge((p.strokes.len() + s.prerequisites.len()) as u64)?;
            if !self.ready(state, i) {
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
                    self.charge(s.widths_mm.len() as u64 + 1)?;
                    let edges = traversal(s, reverse, seam);
                    if !self.deposition_clear(state, s, &edges)? {
                        continue;
                    }
                    let Some(arrival) = self.travel(state, edges[0].0, portals)? else {
                        continue;
                    };
                    let mut step = Step {
                        stroke: i,
                        reverse,
                        seam,
                        travel_mm: arrival,
                        continuous: false,
                    };
                    step.continuous = self.continuous(state, &step);
                    let end = edges.last().unwrap().1;
                    let mut following_distance_mm = f64::INFINITY;
                    for (j, next) in p.strokes.iter().enumerate() {
                        self.charge(1)?;
                        if !state.done(j) && j != i {
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
                        travel_mm: step.travel_mm.windows(2).map(|p| p[0].distance(p[1])).sum(),
                        following_distance_mm,
                        step,
                    });
                }
            }
        }
        Ok(choices)
    }
}
fn run(
    p: &Problem,
    limits: Limits,
    cancel: &AtomicBool,
    solve: impl FnOnce(&mut Session<'_>) -> Result<Route, Error>,
) -> Result<Route, Error> {
    if limits.frontier_states == 0 {
        return Err(Error::Resource);
    }
    let mut session = Session::new(p, limits, cancel);
    validate(p, limits, &mut session.meter)?;
    let mut route = solve(&mut session)?;
    // Final replay has its own equal work allowance: optional search may not
    // consume the reserve needed to validate a previously found incumbent.
    let mut replay = Session::new(p, limits, cancel);
    if route.steps.len() != p.strokes.len() {
        return Err(Error::InvalidInput);
    }
    let mut state = replay.empty();
    for step in &route.steps {
        replay.push(&mut state, step.clone())?;
    }
    route.work = session
        .meter
        .used
        .checked_add(replay.meter.used)
        .ok_or(Error::Resource)?;
    Ok(route)
}
pub fn plan(p: &Problem, limits: Limits, cancel: &AtomicBool) -> Result<Route, Error> {
    run(p, limits, cancel, search::solve)
}
/// Frozen input-order DFS recovery, including checked local-height portals.
pub fn plan_input_order(p: &Problem, limits: Limits, cancel: &AtomicBool) -> Result<Route, Error> {
    run(p, limits, cancel, crate::reference::solve)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn problem() -> Problem {
        Problem {
            strokes: vec![Stroke {
                id: StrokeId(0),
                points_mm: vec![PointMm([0., 0., 0.2]), PointMm([1., 0., 0.2])],
                widths_mm: vec![0.6],
                physical_z_mm: [0., 0.2],
                process: ProcessKey {
                    tool: 0,
                    material: 0,
                    continuity_group: 0,
                },
                prerequisites: vec![],
                allow_reverse: false,
                allow_seam_rotation: false,
            }],
            start_mm: PointMm([0., 0., 0.2]),
            envelope_mm: BoundsMm {
                min: [-2., -2., 0.],
                max: [2., 2., 2.],
            },
            head_mm: vec![BoundsMm {
                min: [-0.1, -0.1, 0.],
                max: [0.1, 0.1, 1.],
            }],
            obstacles_mm: vec![],
            waypoints_mm: vec![],
            travel_speed_mm_s: 10.,
            deposition_speed_mm_s: 10.,
        }
    }
    #[test]
    fn complete_route_replay_rejects_custom_teleport_permissions_and_missing_work() {
        let p = problem();
        let cancel = AtomicBool::new(false);
        let limits = Limits {
            work: 10000,
            edges: 10,
            waypoints: 10,
            frontier_states: 10,
        };
        let valid = plan_input_order(&p, limits, &cancel).unwrap();
        let mut bad = valid.clone();
        bad.steps.clear();
        assert!(matches!(
            run(&p, limits, &cancel, |_| Ok(bad)),
            Err(Error::InvalidInput)
        ));
        let mut bad = valid.clone();
        bad.steps[0].reverse = true;
        assert!(matches!(
            run(&p, limits, &cancel, |_| Ok(bad)),
            Err(Error::InvalidInput)
        ));
        let mut bad = valid.clone();
        bad.steps[0].travel_mm[0] = PointMm([0., 1., 0.2]);
        assert!(matches!(
            run(&p, limits, &cancel, |_| Ok(bad)),
            Err(Error::InvalidInput)
        ));
        let mut bad = valid;
        bad.steps[0].continuous = true;
        assert!(matches!(
            run(&p, limits, &cancel, |_| Ok(bad)),
            Err(Error::InvalidInput)
        ));
    }
    #[test]
    fn returning_to_same_endpoint_does_not_authorize_continuous_deposition() {
        let mut p = problem();
        let mut second = p.strokes[0].clone();
        second.id = StrokeId(1);
        second.points_mm = vec![PointMm([1., 0., 0.2]), PointMm([1., 1., 0.2])];
        p.strokes.push(second);
        let limits = Limits {
            work: 10000,
            edges: 10,
            waypoints: 10,
            frontier_states: 10,
        };
        let cancel = AtomicBool::new(false);
        let valid = plan_input_order(&p, limits, &cancel).unwrap();
        assert!(valid.steps[1].continuous);
        let mut bad = valid.clone();
        bad.steps[1].travel_mm = vec![
            PointMm([1., 0., 0.2]),
            PointMm([1.5, 1., 0.2]),
            PointMm([1., 0., 0.2]),
        ];
        assert!(matches!(
            run(&p, limits, &cancel, |_| Ok(bad)),
            Err(Error::InvalidInput)
        ));
        let mut deliberate_stop = valid;
        deliberate_stop.steps[1].continuous = false;
        assert!(run(&p, limits, &cancel, |_| Ok(deliberate_stop)).is_ok());
    }
    #[test]
    fn clearance_includes_earlier_edges_of_the_same_stroke() {
        let mut p = problem();
        p.head_mm[0].min[2] = -0.1;
        p.strokes[0].points_mm.push(PointMm([1., 1., 0.2]));
        p.strokes[0].widths_mm.push(0.6);
        let limits = Limits {
            work: 10000,
            edges: 10,
            waypoints: 10,
            frontier_states: 10,
        };
        assert!(matches!(
            plan_input_order(&p, limits, &AtomicBool::new(false)),
            Err(Error::NoRoute)
        ));
    }
    #[test]
    fn exhausted_optional_search_keeps_independent_replay_reserve() {
        let p = problem();
        let cancel = AtomicBool::new(false);
        let limits = Limits {
            work: 10000,
            edges: 10,
            waypoints: 10,
            frontier_states: 10,
        };
        let valid = plan_input_order(&p, limits, &cancel).unwrap();
        let result = run(&p, limits, &cancel, |s| {
            assert_eq!(s.charge(s.remaining_work() + 1), Err(Error::Resource));
            Ok(valid)
        })
        .unwrap();
        assert_eq!(result.steps.len(), 1);
        assert!(result.work <= limits.work * 2);
    }
}
