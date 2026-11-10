// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Frozen evaluator. Does not call planner clearance, traversal or search helpers.
use spiling_deposition_routing::*;

#[derive(Debug)]
pub struct Score {
    pub travel_mm: f64,
    pub starts: usize,
    pub deposition_mm: f64,
    pub longest_chain_mm: f64,
    pub intermediate_waypoints: usize,
    pub motion_lower_bound_s: f64,
}
// Independent affine-plane partition: box membership can only change at one
// of the six face crossings. Check every resulting open interval, not a grid.
fn blocked(a: PointMm, b: PointMm, body: BoundsMm, obstacle: BoundsMm) -> bool {
    let lo = std::array::from_fn::<_, 3, _>(|i| obstacle.min[i] - body.max[i]);
    let hi = std::array::from_fn::<_, 3, _>(|i| obstacle.max[i] - body.min[i]);
    let mut cuts = vec![0., 1.];
    for axis in 0..3 {
        let delta = b.0[axis] - a.0[axis];
        if delta != 0. {
            for face in [lo[axis], hi[axis]] {
                let t = (face - a.0[axis]) / delta;
                if t > 0. && t < 1. {
                    cuts.push(t);
                }
            }
        }
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    cuts.windows(2).any(|pair| {
        let t = (pair[0] + pair[1]) * 0.5;
        (0..3).all(|axis| {
            let value = a.0[axis] + t * (b.0[axis] - a.0[axis]);
            value > lo[axis] && value < hi[axis]
        })
    })
}
fn clear(p: &Problem, built: &[BoundsMm], a: PointMm, b: PointMm) -> bool {
    let inside = |q: PointMm| {
        (0..3).all(|i| {
            q.0[i].is_finite() && q.0[i] >= p.envelope_mm.min[i] && q.0[i] <= p.envelope_mm.max[i]
        })
    };
    inside(a)
        && inside(b)
        && p.head_mm.iter().all(|head| {
            p.obstacles_mm
                .iter()
                .chain(built)
                .all(|obstacle| !blocked(a, b, *head, *obstacle))
        })
}
pub fn inspect(p: &Problem, route: &Route) -> Result<Score, &'static str> {
    if route.steps.len() != p.strokes.len() {
        return Err("incomplete inventory");
    }
    let mut seen = vec![false; p.strokes.len()];
    let mut built = Vec::new();
    let mut position = p.start_mm;
    let mut score = Score {
        travel_mm: 0.,
        starts: 0,
        deposition_mm: 0.,
        longest_chain_mm: 0.,
        intermediate_waypoints: 0,
        motion_lower_bound_s: 0.,
    };
    let mut prior: Option<(ProcessKey, [f64; 2], f64)> = None;
    let mut chain = 0.;
    for step in &route.steps {
        let s = p.strokes.get(step.stroke).ok_or("unknown stroke")?;
        if seen[step.stroke] {
            return Err("duplicate stroke");
        }
        if !s.prerequisites.iter().all(|id| {
            p.strokes
                .iter()
                .enumerate()
                .any(|(i, x)| x.id == *id && seen[i])
        }) {
            return Err("missing earlier prerequisite");
        }
        if step.reverse && !s.allow_reverse {
            return Err("locked direction");
        }
        if step.seam > 0 && (!s.allow_seam_rotation || step.seam >= s.widths_mm.len()) {
            return Err("locked seam");
        }
        // Reconstruct rotated/reversed source vertex and width arrays separately.
        let n = s.widths_mm.len();
        let closed = s.points_mm[0] == s.points_mm[n];
        let mut pts = if closed {
            s.points_mm[..n].to_vec()
        } else {
            s.points_mm.clone()
        };
        let mut widths = s.widths_mm.clone();
        if closed {
            pts.rotate_left(step.seam);
            widths.rotate_left(step.seam);
            pts.push(pts[0]);
        }
        if step.reverse {
            pts.reverse();
            widths.reverse();
        }
        if step.travel_mm.first() != Some(&position) || step.travel_mm.last() != Some(&pts[0]) {
            return Err("discontinuous travel binding");
        }
        for line in step.travel_mm.windows(2) {
            if !clear(p, &built, line[0], line[1]) {
                return Err("travel collision/envelope");
            }
            score.travel_mm += line[0].distance(line[1]);
        }
        if step.travel_mm.len() == 1 && !clear(p, &built, position, position) {
            return Err("blocked stationary entry");
        }
        score.intermediate_waypoints += step.travel_mm.len().saturating_sub(2);
        let compatible = position == pts[0]
            && prior.is_some_and(|(key, z, w)| {
                key == s.process && z == s.physical_z_mm && w == widths[0]
            });
        if step.continuous && (!compatible || step.travel_mm.len() != 1) {
            return Err("unauthorized extrusion connection");
        }
        if !step.continuous {
            score.starts += 1;
            chain = 0.;
        }
        for (edge, w) in pts.windows(2).zip(&widths) {
            if !clear(p, &built, edge[0], edge[1]) {
                return Err("deposition head collision");
            }
            let length = edge[0].distance(edge[1]);
            score.deposition_mm += length;
            chain += length;
            score.longest_chain_mm = score.longest_chain_mm.max(chain);
            built.push(BoundsMm {
                min: [
                    edge[0].0[0].min(edge[1].0[0]) - w / 2.,
                    edge[0].0[1].min(edge[1].0[1]) - w / 2.,
                    s.physical_z_mm[0],
                ],
                max: [
                    edge[0].0[0].max(edge[1].0[0]) + w / 2.,
                    edge[0].0[1].max(edge[1].0[1]) + w / 2.,
                    s.physical_z_mm[1],
                ],
            });
        }
        seen[step.stroke] = true;
        position = *pts.last().unwrap();
        prior = Some((s.process, s.physical_z_mm, *widths.last().unwrap()));
    }
    score.motion_lower_bound_s =
        score.travel_mm / p.travel_speed_mm_s + score.deposition_mm / p.deposition_speed_mm_s;
    if !score.travel_mm.is_finite() || !score.motion_lower_bound_s.is_finite() {
        return Err("nonfinite metric");
    }
    Ok(score)
}
