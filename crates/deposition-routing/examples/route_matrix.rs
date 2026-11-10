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
        "case\tsplit\talgorithm\tstatus\ttravel_mm\tstarts\tdeposition_mm\tlongest_chain_mm\twaypoints\tmotion_lower_bound_s\tmachine_time_s\tstarts_bound\ttravel_bound_mm\twork\tplanning_us"
    );
    // Machine-local accelerator policy, recorded so receipts name their device.
    let backend = match std::env::var("SPILING_COMPUTE_BACKEND").as_deref() {
        Ok("auto") => spiling_contracts::compute::ComputeBackend::Auto,
        Ok("vulkan") => spiling_contracts::compute::ComputeBackend::Vulkan,
        Ok("cpu") | Err(_) => spiling_contracts::compute::ComputeBackend::Cpu,
        Ok(_) => return Err("SPILING_COMPUTE_BACKEND must be auto, cpu or vulkan".into()),
    };
    let status = spiling_compute::configure(spiling_contracts::compute::ComputeSettings {
        default_backend: backend,
        fea_backend: None,
    })?;
    eprintln!("compute\t{:?}", status.default_device);
    let cancel = AtomicBool::new(false);
    let mut failed = false;
    // `--sealed SEED` runs the regenerated/transformed promotion panel.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let seed = match arguments.as_slice() {
        [] => 0,
        [flag, value] if flag == "--sealed" => value.parse::<u64>()?.max(1),
        _ => return Err("usage: route_matrix [--sealed SEED]".into()),
    };
    for case in cases::panel(seed) {
        let bound = judge::bounds(&case.problem);
        for (name, router) in [
            (
                "baseline",
                plan_input_order as fn(&Problem, Limits, &AtomicBool) -> Result<Route, Error>,
            ),
            ("candidate", plan),
        ] {
            let start = Instant::now();
            let result = router(&case.problem, limits(&case.problem), &cancel);
            let us = start.elapsed().as_micros();
            match result {
                Ok(route) if case.feasible => {
                    let score = match judge::inspect(&case.problem, &route) {
                        Ok(score) => score,
                        Err(why) => {
                            eprintln!("{} {name}: {why}", case.name);
                            println!(
                                "{}\t{}\t{name}\tinvalid_route\t0\t0\t0\t0\t0\t0\t0\t0\t0\t{}\t{us}",
                                case.name, case.split, route.work
                            );
                            failed = true;
                            continue;
                        }
                    };
                    if score.starts < bound.starts || score.travel_mm < bound.travel_mm - 1e-6 {
                        return Err(
                            "a judged route beat a lower bound; judge or bound is wrong".into()
                        );
                    }
                    println!(
                        "{}\t{}\t{name}\tok\t{:.9}\t{}\t{:.9}\t{:.9}\t{}\t{:.9}\t{:.9}\t{}\t{:.9}\t{}\t{}",
                        case.name,
                        case.split,
                        score.travel_mm,
                        score.starts,
                        score.deposition_mm,
                        score.longest_chain_mm,
                        score.intermediate_waypoints,
                        score.motion_lower_bound_s,
                        score.machine_time_s,
                        bound.starts,
                        bound.travel_mm,
                        route.work,
                        us
                    );
                }
                Err(Error::NoRoute) if !case.feasible => println!(
                    "{}\t{}\t{name}\texpected_refusal\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t{us}",
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
                        "{}\t{}\t{name}\t{status}\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t{us}",
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
/// Caller-sized research allowances scaled to the declared inventory.
fn limits(p: &Problem) -> Limits {
    let points: usize = p.strokes.iter().map(|s| s.points_mm.len()).sum();
    Limits {
        work: 200_000_000_000,
        edges: 100_000,
        waypoints: 512.max(points * 8),
        frontier_states: 128.max(p.strokes.len() + 64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_matrix_and_inaccessible_boundary() {
        for case in cases::matrix() {
            let result = plan_input_order(
                &case.problem,
                limits(&case.problem),
                &AtomicBool::new(false),
            );
            if case.feasible {
                let score = judge::inspect(&case.problem, &result.unwrap()).unwrap();
                let bound = judge::bounds(&case.problem);
                assert!(score.starts >= bound.starts, "{}", case.name);
                assert!(score.travel_mm >= bound.travel_mm - 1e-6, "{}", case.name);
                assert!(score.machine_time_s >= score.motion_lower_bound_s);
            } else {
                assert!(matches!(result, Err(Error::NoRoute)));
            }
        }
    }
    #[test]
    fn bounds_are_exact_on_known_graphs_and_sealed_panels_stay_judged() {
        let cases = cases::matrix();
        let bound =
            |name: &str| judge::bounds(&cases.iter().find(|c| c.name == name).unwrap().problem);
        // One 12-edge path is one trail; twelve separate islands need twelve.
        assert_eq!(bound("chain").starts, 1);
        assert_eq!(bound("islands").starts, 12);
        // A cycle with one leaf has two odd vertices: one trail.
        assert_eq!(bound("junction-cycle").starts, 1);
        assert_eq!(bound("chain").travel_mm, 4f64.hypot(4.).hypot(0.8));
        for seed in [3, 6] {
            let sealed = cases::panel(seed);
            assert_eq!(sealed.len(), cases.len());
            for (case, original) in sealed.iter().zip(&cases) {
                if case.problem.strokes.len() > 80 {
                    continue;
                }
                let result = plan_input_order(
                    &case.problem,
                    limits(&case.problem),
                    &AtomicBool::new(false),
                );
                if case.feasible {
                    let score = judge::inspect(&case.problem, &result.unwrap()).unwrap();
                    if !case.name.starts_with("lattice") {
                        let plain = plan_input_order(
                            &original.problem,
                            limits(&original.problem),
                            &AtomicBool::new(false),
                        )
                        .unwrap();
                        let plain = judge::inspect(&original.problem, &plain).unwrap();
                        assert_eq!(score.starts, plain.starts, "{}", case.name);
                        assert!((score.deposition_mm - plain.deposition_mm).abs() < 1e-6);
                    }
                } else {
                    assert!(matches!(result, Err(Error::NoRoute)));
                }
            }
        }
    }
    #[test]
    fn judge_rejects_deleted_duplicate_locked_and_false_continuation() {
        let cases = cases::matrix();
        let p = &cases.iter().find(|c| c.name == "chain").unwrap().problem;
        let valid = plan_input_order(p, limits(p), &AtomicBool::new(false)).unwrap();
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
        let mut bad = plan_input_order(p, limits(p), &AtomicBool::new(false)).unwrap();
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
        let mut bad = plan_input_order(p, limits(p), &AtomicBool::new(false)).unwrap();
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
        let mut bad = plan_input_order(p, limits(p), &AtomicBool::new(false)).unwrap();
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
            plan(&invalid, limits(&invalid), &cancel),
            Err(Error::InvalidInput)
        ));
        let mut invalid = p.clone();
        invalid.strokes[0].points_mm[0].0[0] = f64::INFINITY;
        assert!(matches!(
            plan(&invalid, limits(&invalid), &cancel),
            Err(Error::InvalidInput)
        ));
        let mut invalid = p.clone();
        invalid.strokes[0].prerequisites.push(StrokeId(u64::MAX));
        assert!(matches!(
            plan(&invalid, limits(&invalid), &cancel),
            Err(Error::InvalidInput)
        ));
        let mut invalid = p;
        invalid.strokes[1].id = invalid.strokes[0].id;
        assert!(matches!(
            plan(&invalid, limits(&invalid), &cancel),
            Err(Error::InvalidInput)
        ));
    }

    #[test]
    fn resource_cancel_and_cycle_are_not_geometric_refusals() {
        let mut p = cases::matrix().remove(0).problem;
        assert!(matches!(
            plan(&p, limits(&p), &AtomicBool::new(true)),
            Err(Error::Cancelled)
        ));
        assert!(matches!(
            plan(
                &p,
                Limits {
                    work: 0,
                    ..limits(&p)
                },
                &AtomicBool::new(false)
            ),
            Err(Error::Resource)
        ));
        p.strokes[0].prerequisites = vec![p.strokes[1].id];
        p.strokes[1].prerequisites = vec![p.strokes[0].id];
        assert!(matches!(
            plan(&p, limits(&p), &AtomicBool::new(false)),
            Err(Error::InvalidInput)
        ));
    }
}
