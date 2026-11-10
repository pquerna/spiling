// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
#[path = "matrix/cases.rs"]
mod cases;
#[path = "matrix/judge.rs"]
mod judge;
use spiling_deposition_routing::*;
use std::{sync::atomic::AtomicBool, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "case\tsplit\talgorithm\tstatus\ttravel_mm\tstarts\tdeposition_mm\tlongest_chain_mm\twaypoints\tmotion_lower_bound_s\twork\tplanning_us"
    );
    let cancel = AtomicBool::new(false);
    let mut failed = false;
    for case in cases::matrix() {
        for (name, router) in [
            (
                "baseline",
                plan_input_order as fn(&Problem, Limits, &AtomicBool) -> Result<Route, Error>,
            ),
            ("candidate", plan),
        ] {
            let start = Instant::now();
            let result = router(
                &case.problem,
                Limits {
                    work: 100_000_000,
                    edges: 100_000,
                    waypoints: 512,
                    frontier_states: 128,
                },
                &cancel,
            );
            let us = start.elapsed().as_micros();
            match result {
                Ok(route) if case.feasible => {
                    let score = match judge::inspect(&case.problem, &route) {
                        Ok(score) => score,
                        Err(why) => {
                            eprintln!("{} {name}: {why}", case.name);
                            println!(
                                "{}\t{}\t{name}\tinvalid_route\t0\t0\t0\t0\t0\t0\t{}\t{us}",
                                case.name, case.split, route.work
                            );
                            failed = true;
                            continue;
                        }
                    };
                    println!(
                        "{}\t{}\t{name}\tok\t{:.9}\t{}\t{:.9}\t{:.9}\t{}\t{:.9}\t{}\t{}",
                        case.name,
                        case.split,
                        score.travel_mm,
                        score.starts,
                        score.deposition_mm,
                        score.longest_chain_mm,
                        score.intermediate_waypoints,
                        score.motion_lower_bound_s,
                        route.work,
                        us
                    );
                }
                Err(Error::NoRoute) if !case.feasible => println!(
                    "{}\t{}\t{name}\texpected_refusal\t0\t0\t0\t0\t0\t0\t0\t{us}",
                    case.name, case.split
                ),
                other => {
                    let status = match &other {
                        Err(Error::NoRoute) => "no_route",
                        Err(Error::Resource) => "resource",
                        Err(Error::Cancelled) => "cancelled",
                        Err(Error::InvalidInput) => "invalid_input",
                        Ok(_) => "unexpected_success",
                    };
                    eprintln!("{} {name}: {status}", case.name);
                    println!(
                        "{}\t{}\t{name}\t{status}\t0\t0\t0\t0\t0\t0\t0\t{us}",
                        case.name, case.split
                    );
                    failed = true;
                }
            }
        }
    }
    if failed {
        Err("matrix contains failed or unknown results".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> Limits {
        Limits {
            work: 100_000_000,
            edges: 100_000,
            waypoints: 512,
            frontier_states: 128,
        }
    }
    #[test]
    fn complete_matrix_and_inaccessible_boundary() {
        for case in cases::matrix() {
            let result = plan_input_order(&case.problem, limits(), &AtomicBool::new(false));
            if case.feasible {
                judge::inspect(&case.problem, &result.unwrap()).unwrap();
            } else {
                assert!(matches!(result, Err(Error::NoRoute)));
            }
        }
    }
    #[test]
    fn judge_rejects_deleted_duplicate_locked_and_false_continuation() {
        let cases = cases::matrix();
        let p = &cases.iter().find(|c| c.name == "chain").unwrap().problem;
        let valid = plan_input_order(p, limits(), &AtomicBool::new(false)).unwrap();
        let mut bad = valid.clone();
        bad.steps.pop();
        assert!(judge::inspect(p, &bad).is_err());
        let mut bad = valid.clone();
        bad.steps[1] = bad.steps[0].clone();
        assert!(judge::inspect(p, &bad).is_err());
        let mut bad = valid;
        bad.steps[0].continuous = true;
        assert!(judge::inspect(p, &bad).is_err());
        let p = &cases
            .iter()
            .find(|c| c.name == "nested-walls")
            .unwrap()
            .problem;
        let mut bad = plan_input_order(p, limits(), &AtomicBool::new(false)).unwrap();
        bad.steps[0].reverse = true;
        assert!(judge::inspect(p, &bad).is_err());
    }
    #[test]
    fn judge_rejects_clear_endpoints_crossing_and_wrong_band_precedence() {
        let cases = cases::matrix();
        let p = &cases
            .iter()
            .find(|c| c.name == "obstacle-lift")
            .unwrap()
            .problem;
        let mut bad = plan_input_order(p, limits(), &AtomicBool::new(false)).unwrap();
        let step = &mut bad.steps[0];
        step.travel_mm = vec![step.travel_mm[0], *step.travel_mm.last().unwrap()];
        assert_eq!(
            judge::inspect(p, &bad).unwrap_err(),
            "travel collision/envelope"
        );
        let p = &cases
            .iter()
            .find(|c| c.name == "fine-coarse")
            .unwrap()
            .problem;
        let mut bad = plan_input_order(p, limits(), &AtomicBool::new(false)).unwrap();
        let coarse = bad
            .steps
            .iter()
            .position(|s| !p.strokes[s.stroke].prerequisites.is_empty())
            .unwrap();
        bad.steps.swap(0, coarse);
        assert_eq!(
            judge::inspect(p, &bad).unwrap_err(),
            "missing earlier prerequisite"
        );
    }
    #[test]
    fn invalid_dimensions_coordinates_and_identity_refuse() {
        let p = cases::matrix().remove(0).problem;
        let cancel = AtomicBool::new(false);
        let mut invalid = p.clone();
        invalid.strokes[0].widths_mm[0] = f64::NAN;
        assert!(matches!(
            plan(&invalid, limits(), &cancel),
            Err(Error::InvalidInput)
        ));
        let mut invalid = p.clone();
        invalid.strokes[0].points_mm[0].0[0] = f64::INFINITY;
        assert!(matches!(
            plan(&invalid, limits(), &cancel),
            Err(Error::InvalidInput)
        ));
        let mut invalid = p.clone();
        invalid.strokes[0].prerequisites.push(StrokeId(u64::MAX));
        assert!(matches!(
            plan(&invalid, limits(), &cancel),
            Err(Error::InvalidInput)
        ));
        let mut invalid = p;
        invalid.strokes[1].id = invalid.strokes[0].id;
        assert!(matches!(
            plan(&invalid, limits(), &cancel),
            Err(Error::InvalidInput)
        ));
    }

    #[test]
    fn resource_cancel_and_cycle_are_not_geometric_refusals() {
        let mut p = cases::matrix().remove(0).problem;
        assert!(matches!(
            plan(&p, limits(), &AtomicBool::new(true)),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            plan(
                &p,
                Limits {
                    work: 0,
                    ..limits()
                },
                &AtomicBool::new(false)
            ),
            Err(Error::Resource)
        ));
        p.strokes[0].prerequisites = vec![p.strokes[1].id];
        p.strokes[1].prerequisites = vec![p.strokes[0].id];
        assert!(matches!(
            plan(&p, limits(), &AtomicBool::new(false)),
            Err(Error::InvalidInput)
        ));
    }
}
