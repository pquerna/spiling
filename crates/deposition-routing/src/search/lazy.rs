// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Lazy nearest-entry construction. Straight distance is an admissible bound
//! on checked travel, so candidates are priced in bound order and pricing
//! stops once the bound cannot beat the best checked travel. Returns None on
//! a dead end so the caller can fall back to complete backtracking.
//! Band-major: only the lowest physical band with unfinished work is eligible,
//! so later nozzle passes never approach finished higher material.
use crate::{planner::Session, *};
pub(super) fn solve(session: &mut Session<'_>) -> Result<Option<Route>, Error> {
    let p = session.problem;
    let index = |id: StrokeId| p.strokes.iter().position(|s| s.id == id).unwrap();
    let prerequisites: Vec<Vec<usize>> = p
        .strokes
        .iter()
        .map(|s| s.prerequisites.iter().map(|id| index(*id)).collect())
        .collect();
    let mut state = session.empty();
    for _ in 0..p.strokes.len() {
        let lowest = (0..p.strokes.len())
            .filter(|i| !state.done(*i))
            .map(|i| p.strokes[i].physical_z_mm[1])
            .fold(f64::INFINITY, f64::min);
        let mut candidates = Vec::new();
        for (i, s) in p.strokes.iter().enumerate() {
            session.charge(1)?;
            if state.done(i)
                || s.physical_z_mm[1] != lowest
                || !prerequisites[i].iter().all(|q| state.done(*q))
            {
                continue;
            }
            let seams = if s.allow_seam_rotation {
                s.widths_mm.len()
            } else {
                1
            };
            for reverse in [false, true]
                .into_iter()
                .take(1 + usize::from(s.allow_reverse))
            {
                for seam in 0..seams {
                    let entry = traversal(s, reverse, seam)[0].0;
                    candidates.push((state.position.distance(entry), i, reverse, seam, entry));
                }
            }
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut priced = Vec::new();
        let mut best = f64::INFINITY;
        for &(bound, stroke, reverse, seam, entry) in &candidates {
            if bound > best {
                break;
            }
            if let Some(path) = session.travel(&state, entry, &[])? {
                let cost: f64 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
                best = best.min(cost);
                priced.push((cost, stroke, reverse, seam, path));
            }
        }
        priced.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut admitted = false;
        'push: for (_, stroke, reverse, seam, path) in priced {
            for continuous in [path.len() == 1, false] {
                let step = Step {
                    stroke,
                    reverse,
                    seam,
                    travel_mm: path.clone(),
                    continuous,
                };
                match session.push(&mut state, step) {
                    Ok(()) => {
                        admitted = true;
                        break 'push;
                    }
                    Err(Error::InvalidInput) => {}
                    Err(other) => return Err(other),
                }
            }
        }
        if !admitted {
            return Ok(None);
        }
    }
    Ok(Some(state.route()))
}
