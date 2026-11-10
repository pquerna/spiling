// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;

const NONE: &[u32] = &[];
fn line(x: f64, y: f64, z: f64, tool: u32, barrier: u32) -> Task<'static> {
    Task {
        ends: Ends::Open {
            first: [x, y, z],
            last: [x + 4., y, z],
            reversible: true,
        },
        tool,
        barrier,
        prerequisites: NONE,
        duration_s: 1.,
        seam_policy: None,
        physical_z_mm: [z - 0.2, z],
    }
}
fn machine(tools: u32, service: Service) -> Machine {
    Machine {
        start: Some([0., 0., 0.2]),
        initial_tool: None,
        tools,
        travel: Travel {
            speed_mm_s: 100.,
            lift_mm: 0.,
            retraction: None,
        },
        change: ToolChange {
            swap_s: 7.,
            prewarm_lead_s: None,
            service_s: 0.,
            service,
            stations_xy: vec![None; tools as usize],
        },
    }
}
const BUDGET: Budget = Budget { work: 10_000_000 };
fn run(tasks: &[Task<'_>], machine: &Machine, order: ToolOrder) -> Schedule {
    schedule(tasks, machine, order, BUDGET, &AtomicBool::new(false)).unwrap()
}

#[test]
fn every_task_is_scheduled_once_and_search_beats_scrambled_input() {
    let tasks: Vec<_> = [5, 0, 3, 1, 4, 2, 7, 6]
        .into_iter()
        .map(|i| line(i as f64 * 4., 0., 0.2, 0, 0))
        .collect();
    let m = machine(1, Service::WhenUsed);
    let control = schedule_input_order(&tasks, &m, &AtomicBool::new(false)).unwrap();
    let routed = run(&tasks, &m, ToolOrder::Free);
    let mut seen: Vec<u32> = routed.visits.iter().map(|v| v.task).collect();
    seen.sort_unstable();
    assert_eq!(seen, (0..8).collect::<Vec<_>>());
    // One exact chain from the start: no travel at all.
    assert_eq!(routed.cost.travel_mm, 0.);
    assert_eq!(routed.cost.starts, 1);
    assert!(control.cost.travel_mm > 50.);
    assert_eq!(routed, run(&tasks, &m, ToolOrder::Free));
}

#[test]
fn spent_allowance_still_returns_a_complete_checked_schedule_and_cancel_aborts() {
    let tasks: Vec<_> = (0..20)
        .map(|i| line(0., (i * 7 % 20) as f64, 0.2, 0, 0))
        .collect();
    let m = machine(1, Service::WhenUsed);
    let none = schedule(
        &tasks,
        &m,
        ToolOrder::Free,
        Budget { work: 0 },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(none.visits.len(), 20);
    assert_eq!(none.work, 0);
    assert!(run(&tasks, &m, ToolOrder::Free).cost.total_s <= none.cost.total_s);
    assert_eq!(
        schedule(&tasks, &m, ToolOrder::Free, BUDGET, &AtomicBool::new(true)),
        Err(Error::Cancelled)
    );
}

#[test]
fn locked_direction_fixed_seams_and_prerequisites_are_honoured() {
    let seams = [[10., 0., 0.2], [10., 5., 0.2]];
    let needs_loop = [1u32];
    let tasks = [
        Task {
            ends: Ends::Open {
                first: [9., 0., 0.2],
                last: [1., 0., 0.2],
                reversible: false,
            },
            prerequisites: &needs_loop,
            ..line(0., 0., 0.2, 0, 0)
        },
        Task {
            ends: Ends::Loop { seams: &seams },
            ..line(0., 0., 0.2, 0, 0)
        },
    ];
    let routed = run(&tasks, &machine(1, Service::WhenUsed), ToolOrder::Free);
    assert_eq!(routed.visits[0].task, 1, "prerequisite loop first");
    assert_eq!(routed.visits[0].seam, 0, "nearest permitted seam");
    assert!(!routed.visits[1].reverse, "locked direction kept");
}

#[test]
fn rigid_dual_carriage_services_both_tools_each_barrier_and_free_order_alternates() {
    // Tool 1 has no work in barrier 1 but is still serviced there.
    let tasks = [
        line(0., 0., 0.2, 0, 0),
        line(0., 9., 0.2, 1, 0),
        line(0., 0., 0.4, 0, 1),
        line(0., 0., 0.6, 0, 2),
        line(0., 9., 0.6, 1, 2),
    ];
    let mut m = machine(2, Service::EveryBarrier);
    m.change.stations_xy = vec![Some([-5., 0.]), Some([-5., 9.])];
    m.change.service_s = 2.;
    m.initial_tool = Some(0);
    let fixed = run(&tasks, &m, ToolOrder::Fixed);
    let tools = |s: &Schedule| s.groups.iter().map(|g| g.tool).collect::<Vec<_>>();
    assert_eq!(tools(&fixed), [0, 1, 0, 1, 0, 1]);
    assert_eq!(fixed.cost.selections, 5);
    let free = run(&tasks, &m, ToolOrder::Free);
    assert_eq!(tools(&free), [0, 1, 1, 0, 0, 1]);
    assert_eq!(free.cost.selections, 3);
    assert_eq!(free.groups.len(), 6, "service is never dropped");
    assert!(free.groups[2].visits.is_empty());
    assert_eq!(free.cost.change_s, 3. * 7. + 6. * 2.);
    // A fixed-order check refuses the alternating schedule.
    assert_eq!(
        check(&tasks, &m, ToolOrder::Fixed, &free),
        Err(CheckError::ToolOrder)
    );
}

#[test]
fn three_tool_changer_orders_tools_and_places_prewarm_on_the_clock() {
    let tasks = [
        line(0., 0., 0.2, 0, 0),
        line(0., 9., 0.2, 2, 0),
        line(0., 0., 0.4, 2, 1),
        line(0., 9., 0.4, 1, 1),
        line(0., 0., 0.6, 1, 2),
        line(0., 9., 0.6, 0, 2),
    ];
    let mut m = machine(3, Service::WhenUsed);
    m.initial_tool = Some(0);
    m.change.prewarm_lead_s = Some(4.);
    let routed = run(&tasks, &m, ToolOrder::Free);
    let tools: Vec<_> = routed.groups.iter().map(|g| g.tool).collect();
    assert_eq!(tools, [0, 2, 2, 1, 1, 0]);
    assert_eq!(
        routed.cost.selections, 3,
        "one change per barrier boundary pair"
    );
    let fixed = run(&tasks, &m, ToolOrder::Fixed);
    assert_eq!(fixed.cost.selections, 5);
    for group in &routed.groups {
        match (group.selected, group.prewarm_at_s) {
            (true, Some(at)) => {
                let selection = group.begins_at_s - m.change.swap_s;
                assert!((at - (selection - 4.).max(0.)).abs() < 1e-12);
            }
            (false, None) => {}
            other => panic!("pre-warm must accompany exactly each selection: {other:?}"),
        }
    }
}

#[test]
fn cross_tool_prerequisites_constrain_tool_order_or_refuse() {
    let needs_first = [0u32];
    let needs_second = [1u32];
    let mut tasks = [
        line(0., 0., 0.2, 1, 0),
        Task {
            prerequisites: &needs_first,
            ..line(0., 9., 0.2, 0, 0)
        },
    ];
    let m = machine(2, Service::WhenUsed);
    let free = run(&tasks, &m, ToolOrder::Free);
    assert_eq!(
        free.groups.iter().map(|g| g.tool).collect::<Vec<_>>(),
        [1, 0]
    );
    let fixed = schedule(
        &tasks,
        &m,
        ToolOrder::Fixed,
        BUDGET,
        &AtomicBool::new(false),
    );
    assert_eq!(fixed, Err(Error::Inconsistent));
    tasks[0].prerequisites = &needs_second;
    assert_eq!(
        schedule(&tasks, &m, ToolOrder::Free, BUDGET, &AtomicBool::new(false)),
        Err(Error::Inconsistent)
    );
    let later = [1u32];
    let tasks = [
        Task {
            prerequisites: &later,
            ..line(0., 0., 0.2, 0, 0)
        },
        line(0., 0., 0.4, 0, 1),
    ];
    assert_eq!(
        schedule(&tasks, &m, ToolOrder::Free, BUDGET, &AtomicBool::new(false)),
        Err(Error::Inconsistent)
    );
}

#[test]
fn checker_refuses_each_kind_of_bad_schedule() {
    let needs_first = [0u32];
    let tasks = [
        Task {
            ends: Ends::Open {
                first: [0., 0., 0.2],
                last: [4., 0., 0.2],
                reversible: false,
            },
            ..line(0., 0., 0.2, 0, 0)
        },
        Task {
            prerequisites: &needs_first,
            ..line(8., 0., 0.2, 0, 0)
        },
        line(0., 0., 0.4, 0, 1),
    ];
    let m = machine(1, Service::WhenUsed);
    let good = run(&tasks, &m, ToolOrder::Free);
    let refuse = |edit: &dyn Fn(&mut Schedule)| {
        let mut bad = good.clone();
        edit(&mut bad);
        check(&tasks, &m, ToolOrder::Free, &bad).unwrap_err()
    };
    assert_eq!(
        refuse(&|s| s.visits[1] = s.visits[0]),
        CheckError::Inventory
    );
    assert_eq!(
        refuse(&|s| {
            s.visits.pop();
        }),
        CheckError::Inventory
    );
    assert_eq!(
        refuse(&|s| s.visits[0].reverse = true),
        CheckError::Permission
    );
    assert_eq!(refuse(&|s| s.visits[0].seam = 1), CheckError::Permission);
    assert_eq!(refuse(&|s| s.visits.swap(0, 1)), CheckError::Prerequisite);
    assert_eq!(refuse(&|s| s.visits.swap(1, 2)), CheckError::Grouping);
    assert_eq!(refuse(&|s| s.groups.swap(0, 1)), CheckError::Grouping);
    assert_eq!(refuse(&|s| s.cost.travel_mm -= 1.), CheckError::Annotation);
    assert_eq!(
        refuse(&|s| s.groups[1].selected = true),
        CheckError::Annotation
    );
    assert_eq!(refuse(&|s| s.cost.selections = 0), CheckError::Annotation);
}

#[test]
fn travel_model_prices_lift_and_thresholded_retraction() {
    let mut travel = Travel {
        speed_mm_s: 100.,
        lift_mm: 1.,
        retraction: Some(Retraction {
            minimum_mm: 2.,
            seconds: 0.05,
            lift_mm: 0.5,
        }),
    };
    let a = [0., 0., 0.2];
    assert_eq!(gap_seconds(&travel, a, a), 0.);
    assert_eq!(gap_seconds(&travel, a, [1., 0., 0.2]), 3. / 100.);
    assert!((gap_seconds(&travel, a, [2., 0., 0.2]) - 0.1).abs() < 1e-12);
    travel.speed_mm_s = f64::NAN;
    let mut m = machine(1, Service::WhenUsed);
    m.travel = travel;
    assert!(matches!(
        schedule(
            &[line(0., 0., 0.2, 0, 0)],
            &m,
            ToolOrder::Free,
            BUDGET,
            &AtomicBool::new(false)
        ),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn authored_wall_seams_use_command_coordinates_and_reject_tampering() {
    let points = [
        [1.0000004, 1., 0.2],
        [3., 3., 0.2],
        [1.0000001, 1., 0.2],
        [4., 4., 0.2],
    ];
    let task = Task {
        ends: Ends::Loop { seams: &points },
        seam_policy: Some(SeamPolicy::Distributed),
        ..line(0., 0., 0.2, 0, 0)
    };
    let mut m = machine(1, Service::WhenUsed);
    m.start = None;
    let result = run(&[task], &m, ToolOrder::Fixed);
    assert_eq!(result.visits[0].seam, 2);
    let mut changed = result.clone();
    changed.visits[0].seam = 0;
    assert_eq!(
        check(&[task], &m, ToolOrder::Fixed, &changed),
        Err(CheckError::Permission)
    );
    assert_eq!(
        schedule(
            &[task],
            &m,
            ToolOrder::Fixed,
            BUDGET,
            &AtomicBool::new(true)
        ),
        Err(Error::Cancelled)
    );
}

#[test]
fn larger_tool_inventory_respects_cross_tool_dependencies() {
    let prerequisite = [6];
    let mut tasks: Vec<_> = (0..7)
        .map(|tool| line(tool as f64, 0., 0.2, tool, 0))
        .collect();
    tasks[0].prerequisites = &prerequisite;
    let result = run(&tasks, &machine(7, Service::EveryBarrier), ToolOrder::Free);
    let a = result.groups.iter().position(|g| g.tool == 6).unwrap();
    let b = result.groups.iter().position(|g| g.tool == 0).unwrap();
    assert!(a < b);
}

#[test]
fn physical_intervals_are_independent_of_barriers_and_bad_input_never_panics() {
    let tasks = [
        line(0., 0., 0.2, 0, 0),
        Task {
            physical_z_mm: [0., 0.6],
            ..line(0., 0., 0.6, 1, 1)
        },
    ];
    let m = machine(2, Service::EveryBarrier);
    let result = run(&tasks, &m, ToolOrder::Free);
    assert_eq!(result.visits.len(), 2);
    let mut bad = tasks;
    bad[1].physical_z_mm = [0., 0.2];
    assert!(matches!(
        schedule(&bad, &m, ToolOrder::Free, BUDGET, &AtomicBool::new(false)),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        check(&bad, &m, ToolOrder::Free, &result),
        Err(CheckError::Input)
    );
    bad = tasks;
    bad[0].prerequisites = &[999];
    assert_eq!(
        check(&bad, &m, ToolOrder::Free, &result),
        Err(CheckError::Input)
    );
}

#[test]
fn first_prewarm_cannot_claim_an_unavailable_lead_time() {
    let tasks = [line(0., 0., 0.2, 0, 0)];
    let mut m = machine(1, Service::WhenUsed);
    m.change.prewarm_lead_s = Some(20.);
    let result = run(&tasks, &m, ToolOrder::Free);
    assert_eq!(result.cost.warmup_s, 20.);
    assert_eq!(result.groups[0].prewarm_at_s, Some(0.));
    assert_eq!(result.groups[0].begins_at_s, 27.);
}
