// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
#[path = "matrix/cases.rs"]
mod cases;
#[path = "matrix/judge.rs"]
mod judge;
use spiling_deposition_routing::*;
use std::{sync::atomic::AtomicBool, time::Instant};

/// Frozen research allowance; production sizes its own.
const BUDGET: Budget = Budget {
    work: 2_000_000_000,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "case\tsplit\talgorithm\tstatus\ttravel_mm\tstarts\tdeposition_mm\tselections\tmotion_lower_bound_s\tmachine_time_s\tstarts_bound\ttravel_bound_mm\twork\tplanning_us"
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
    // `--sealed SEED` runs the regenerated/transformed promotion panel.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let seed = match arguments.as_slice() {
        [] => 0,
        [flag, value] if flag == "--sealed" => value.parse::<u64>()?.max(1),
        _ => return Err("usage: route_matrix [--sealed SEED]".into()),
    };
    let cancel = AtomicBool::new(false);
    let mut failed = false;
    for case in cases::panel(seed) {
        let tasks = case.tasks();
        let bound = judge::bounds(&case);
        for name in ["baseline", "candidate"] {
            let start = Instant::now();
            let result = if name == "baseline" {
                schedule_input_order(&tasks, &case.machine, &cancel)
            } else {
                schedule(&tasks, &case.machine, case.order, BUDGET, &cancel)
            };
            let us = start.elapsed().as_micros();
            let blank = |status: &str, work: u64| {
                println!(
                    "{}\t{}\t{name}\t{status}\t0\t0\t0\t0\t0\t0\t0\t0\t{work}\t{us}",
                    case.name, case.split
                );
            };
            match result {
                Ok(schedule) if case.feasible => {
                    let judged = judge::inspect(&case, &schedule);
                    let score = match judged {
                        Ok(score) => score,
                        Err(why) => {
                            eprintln!("{} {name}: {why}", case.name);
                            blank("invalid_route", schedule.work);
                            failed = true;
                            continue;
                        }
                    };
                    if score.starts < bound.starts || score.travel_mm < bound.travel_mm - 1e-6 {
                        return Err(
                            "a judged schedule beat a lower bound; judge or bound is wrong".into(),
                        );
                    }
                    println!(
                        "{}\t{}\t{name}\tok\t{:.9}\t{}\t{:.9}\t{}\t{:.9}\t{:.9}\t{}\t{:.9}\t{}\t{}",
                        case.name,
                        case.split,
                        score.travel_mm,
                        score.starts,
                        score.deposition_mm,
                        score.selections,
                        score.motion_lower_bound_s,
                        score.machine_time_s,
                        bound.starts,
                        bound.travel_mm,
                        schedule.work,
                        us
                    );
                }
                Err(Error::Inconsistent) if !case.feasible => blank("expected_refusal", 0),
                other => {
                    let status = match &other {
                        Err(Error::Inconsistent) => "no_route",
                        Err(Error::Cancelled) => "cancelled",
                        Err(Error::InvalidInput(_)) => "invalid_input",
                        Err(Error::Rejected(_)) => "invalid_route",
                        Ok(_) => "unexpected_success",
                    };
                    eprintln!("{} {name}: {status}", case.name);
                    blank(status, 0);
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
    fn run(case: &cases::Case) -> Result<Schedule, Error> {
        schedule(
            &case.tasks(),
            &case.machine,
            case.order,
            BUDGET,
            &AtomicBool::new(false),
        )
    }
    fn named(name: &str) -> cases::Case {
        cases::matrix()
            .into_iter()
            .find(|c| c.name == name)
            .unwrap()
    }
    #[test]
    fn whole_panel_is_judged_valid_within_bounds_and_preserves_deposition() {
        for case in cases::matrix() {
            let tasks = case.tasks();
            let control = schedule_input_order(&tasks, &case.machine, &AtomicBool::new(false));
            let routed = run(&case);
            if !case.feasible {
                assert_eq!(routed.unwrap_err(), Error::Inconsistent, "{}", case.name);
                assert_eq!(control.unwrap_err(), Error::Inconsistent);
                continue;
            }
            let score = judge::inspect(&case, &routed.unwrap())
                .unwrap_or_else(|e| panic!("{}: {e}", case.name));
            let bound = judge::bounds(&case);
            assert!(score.starts >= bound.starts, "{}", case.name);
            assert!(score.travel_mm >= bound.travel_mm - 1e-6, "{}", case.name);
            assert!(score.machine_time_s >= score.motion_lower_bound_s - 1e-9);
            let control = judge::inspect(&case, &control.unwrap()).unwrap();
            assert!((score.deposition_mm - control.deposition_mm).abs() < 1e-9);
        }
    }
    #[test]
    fn bounds_are_exact_on_known_graphs() {
        assert_eq!(judge::bounds(&named("chain")).starts, 1);
        assert_eq!(judge::bounds(&named("islands")).starts, 12);
        assert_eq!(judge::bounds(&named("locked-cycle")).starts, 1);
        assert_eq!(judge::bounds(&named("nested-loops")).starts, 3);
        assert_eq!(judge::bounds(&named("chain")).travel_mm, 0.);
    }
    #[test]
    fn multi_tool_machines_keep_service_and_reduce_selections() {
        for name in ["dual-carriage", "changer-three"] {
            let case = named(name);
            let tasks = case.tasks();
            let free = judge::inspect(&case, &run(&case).unwrap()).unwrap();
            let fixed = schedule(
                &tasks,
                &case.machine,
                ToolOrder::Fixed,
                BUDGET,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(free.selections < fixed.cost.selections as usize, "{name}");
        }
        let case = named("dual-carriage");
        let routed = run(&case).unwrap();
        let barriers = case.strokes.iter().map(|s| s.barrier).max().unwrap() as usize + 1;
        assert_eq!(
            routed.groups.len(),
            barriers * 2,
            "every tool serviced on every layer"
        );
        assert!(routed.groups.iter().any(|g| g.visits.is_empty()));
        let case = named("changer-three");
        let routed = run(&case).unwrap();
        assert!(
            routed
                .groups
                .iter()
                .all(|g| g.selected == g.prewarm_at_s.is_some())
        );
    }
    #[test]
    fn judge_rejects_each_mutation_independently() {
        let case = named("dual-carriage");
        let good = run(&case).unwrap();
        judge::inspect(&case, &good).unwrap();
        let refuse = |edit: &dyn Fn(&mut Schedule)| {
            let mut bad = good.clone();
            edit(&mut bad);
            judge::inspect(&case, &bad).unwrap_err()
        };
        assert_eq!(
            refuse(&|s| {
                s.visits.pop();
            }),
            "incomplete inventory"
        );
        assert_eq!(refuse(&|s| s.visits[1] = s.visits[0]), "duplicate task");
        assert_eq!(refuse(&|s| s.visits[0].seam = 1), "locked seam");
        let empty = good
            .groups
            .iter()
            .position(|g| g.visits.is_empty())
            .unwrap();
        assert_eq!(
            refuse(&|s| {
                s.groups.remove(empty);
            }),
            "a required service group is missing"
        );
        assert_eq!(
            refuse(&|s| s.cost.selections -= 1),
            "reported cost differs from independent evaluation"
        );
        assert_eq!(
            refuse(&|s| s.cost.total_s -= 1.),
            "reported cost differs from independent evaluation"
        );
        assert_eq!(
            refuse(&|s| s.groups[0].selected = !s.groups[0].selected),
            "tool transition clock annotation differs"
        );
        assert_eq!(
            refuse(&|s| s.groups[0].begins_at_s += 1.),
            "tool transition clock annotation differs"
        );
        let changer = named("changer-three");
        let mut late = run(&changer).unwrap();
        late.groups
            .iter_mut()
            .find(|g| g.prewarm_at_s.is_some())
            .unwrap()
            .prewarm_at_s = Some(999.);
        assert_eq!(
            judge::inspect(&changer, &late).unwrap_err(),
            "tool transition clock annotation differs"
        );
        let last = good.visits.len() - 1;
        assert_eq!(
            refuse(&|s| s.visits.swap(0, last)),
            "task outside its tool group or barrier"
        );
        let locked = named("locked-cycle");
        let mut bad = run(&locked).unwrap();
        bad.visits[0].reverse = true;
        assert_eq!(
            judge::inspect(&locked, &bad).unwrap_err(),
            "locked direction"
        );
        let stack = named("slicer-pin-stack");
        let mut bad = run(&stack).unwrap();
        let dependent = stack
            .strokes
            .iter()
            .position(|s| !s.prerequisites.is_empty())
            .unwrap() as u32;
        let (a, b) = (
            bad.visits.iter().position(|v| v.task == dependent).unwrap(),
            bad.visits
                .iter()
                .position(|v| v.task == stack.strokes[dependent as usize].prerequisites[0])
                .unwrap(),
        );
        bad.visits.swap(a, b);
        assert!(judge::inspect(&stack, &bad).is_err());
    }
    #[test]
    fn sealed_panels_keep_inventory_and_stay_judged() {
        let visible = cases::matrix();
        for seed in [3, 6] {
            let sealed = cases::panel(seed);
            assert_eq!(sealed.len(), visible.len());
            for (case, original) in sealed.iter().zip(&visible).filter(|(c, _)| c.feasible) {
                let score = judge::inspect(case, &run(case).unwrap()).unwrap();
                if !["lattice", "dual", "changer"]
                    .iter()
                    .any(|p| case.name.starts_with(p))
                {
                    let plain = judge::inspect(original, &run(original).unwrap()).unwrap();
                    assert!(
                        (score.deposition_mm - plain.deposition_mm).abs() < 1e-6,
                        "{}",
                        case.name
                    );
                }
            }
        }
    }
}
