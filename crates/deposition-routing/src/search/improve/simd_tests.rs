use super::*;

fn travel() -> Travel {
    Travel {
        speed_mm_s: 137.0,
        lift_mm: 0.25,
        retraction: Some(Retraction {
            minimum_mm: 2.0,
            seconds: 0.045,
            lift_mm: 0.2,
        }),
    }
}

fn task(ends: Ends<'_>) -> Task<'_> {
    Task {
        ends,
        tool: 0,
        barrier: 0,
        layer: 0,
        prerequisites: &[],
        duration_s: 1.0,
        seam_policy: None,
        physical_z_mm: [0.0, 0.2],
    }
}

fn open(first: PointMm, last: PointMm, reversible: bool) -> Task<'static> {
    task(Ends::Open {
        first,
        last,
        reversible,
    })
}

#[test]
fn price_bits_match_at_thresholds_underflow_and_tail() {
    let pricing = Pricing::new(&travel());
    let Some(backend) = simd::Backend::detect(pricing) else {
        return;
    };
    let xs: [f64; 39] = std::array::from_fn(|ordinal| match ordinal % 8 {
        0 => -0.0,
        1 => 1e-200,
        2 => 2.0_f64.next_down(),
        3 => 2.0,
        4 => 2.0_f64.next_up(),
        5 => -2.0,
        _ => ordinal as f64 * -0.043,
    });
    let ys = std::array::from_fn::<_, 39, _>(|ordinal| {
        if ordinal % 8 >= 6 {
            ordinal as f64 * 0.178
        } else {
            0.0
        }
    });
    let zs = std::array::from_fn::<_, 39, _>(|ordinal| {
        if ordinal % 8 >= 6 {
            ordinal as f64 * -0.154
        } else {
            0.0
        }
    });
    let mut result = [0.; 39];
    backend.price(pricing, [&xs, &ys, &zs], [0.; 3], &mut result);
    for (ordinal, value) in result.into_iter().enumerate() {
        let scalar = pricing.seconds([xs[ordinal], ys[ordinal], zs[ordinal]], [0.; 3]);
        assert_eq!(value.to_bits(), scalar.to_bits(), "lane {ordinal}");
    }
}

#[test]
fn first_ordinal_and_forward_win_ties_across_blocks_and_tail() {
    let size = 71;
    let tasks = vec![open([0.; 3], [0.; 3], true); size];
    let members: Vec<_> = (0..size as u32).collect();
    let travel = travel();
    let cancel = AtomicBool::new(false);
    let mut order = Order::new(
        &tasks,
        &travel,
        &members,
        Some([0.; 3]),
        members.iter().map(|&local| (local, false, 0)).collect(),
        vec![vec![]; size],
        vec![vec![]; size],
    );
    if !order.enable_simd(&cancel) {
        return;
    }
    order.incoming.fill(4.0);
    let mut scratch = Scratch::new(size);
    scratch.left.fill(5.0);
    scratch.flipped.fill(5.0);
    for ordinal in [0, 31, 32, 70] {
        scratch.left[ordinal] = 1.0;
        scratch.flipped[ordinal] = 1.0;
    }
    let best = |scratch: &Scratch| {
        let mut result = (1e-9, usize::MAX, false);
        order.scan_insertions(scratch, 0..size, 0., 69, 69, true, &mut result);
        result
    };
    assert_eq!(best(&scratch), (3., 0, false));
    scratch.flipped[31] = 0.5;
    scratch.left[32] = 0.5;
    scratch.left[70] = 0.5;
    assert_eq!(best(&scratch), (3.5, 31, true));
}

#[test]
fn all_kicks_match_order_orientation_cost_and_work() {
    let size = 79;
    let loops: Vec<_> = (0..size)
        .map(|ordinal| {
            let x = ((ordinal * 13) % 31) as f64;
            let y = ((ordinal * 5) % 17) as f64;
            [[x, y, 0.2], [x + 0.21, y + 0.13, 0.2], [x + 0.3, y, 0.2]]
        })
        .collect();
    let tasks: Vec<_> = loops
        .iter()
        .enumerate()
        .map(|(ordinal, points)| {
            if ordinal % 5 == 0 {
                task(Ends::Loop { seams: points })
            } else {
                open(points[0], points[1], ordinal % 3 != 0)
            }
        })
        .collect();
    let members: Vec<_> = (0..size as u32).collect();
    let mut prerequisites = vec![vec![]; size];
    let mut dependents = vec![vec![]; size];
    for ordinal in (7..size).step_by(7) {
        prerequisites[ordinal].push((ordinal - 3) as u32);
        dependents[ordinal - 3].push(ordinal as u32);
    }
    let machine = Machine {
        start: None,
        initial_tool: Some(0),
        tools: 1,
        travel: travel(),
        change: ToolChange {
            swap_s: 0.,
            prewarm_lead_s: None,
            service_s: 0.,
            service: Service::WhenUsed,
            stations_xy: vec![None],
        },
    };
    let cancel = AtomicBool::new(false);
    for start in [None, Some([7.0, 5.0, 0.1])] {
        let order = || {
            Order::new(
                &tasks,
                &machine.travel,
                &members,
                start,
                members.iter().map(|&local| (local, false, 0)).collect(),
                prerequisites.clone(),
                dependents.clone(),
            )
        };
        let context = || Context {
            tasks: &tasks,
            machine: &machine,
            order: ToolOrder::Free,
            cancel: &cancel,
            work: 0,
        };
        let (mut original, mut vector) = (order(), order());
        let enabled = vector.enable_simd(&cancel);
        #[cfg(target_arch = "x86_64")]
        assert_eq!(enabled, std::is_x86_feature_detected!("avx2"));
        #[cfg(not(target_arch = "x86_64"))]
        assert!(!enabled);
        let (mut original_context, mut vector_context) = (context(), context());
        improve(&mut original_context, &mut original).unwrap();
        improve(&mut vector_context, &mut vector).unwrap();
        assert_eq!(original_context.work, vector_context.work);
        assert_eq!(original.visits, vector.visits);
        assert_eq!(original.cost().to_bits(), vector.cost().to_bits());
        let saved = original.visits.clone();
        let mut original_rng = 0x2545_F491_4F6C_DD1D_u64;
        let mut vector_rng = original_rng;
        for round in 0..8 {
            if round == 4 {
                original.visits.clone_from(&saved);
                vector.visits.clone_from(&saved);
                original.refresh();
                vector.refresh();
            }
            let mut seeds = Vec::new();
            for _ in 0..3 {
                let expected = original.kick(&mut original_rng);
                assert_eq!(expected, vector.kick(&mut vector_rng));
                assert_eq!(original.visits, vector.visits);
                seeds.extend(expected);
            }
            original.descend(&mut original_context, &seeds).unwrap();
            vector.descend(&mut vector_context, &seeds).unwrap();
            assert_eq!(original_context.work, vector_context.work, "kick {round}");
            assert_eq!(original.visits, vector.visits, "kick {round}");
            assert_eq!(
                original.cost().to_bits(),
                vector.cost().to_bits(),
                "kick {round}"
            );
        }
    }
}

#[test]
fn invalid_or_extreme_inputs_and_precancellation_decline_simd() {
    let size = simd::BLOCK;
    let members: Vec<_> = (0..size as u32).collect();
    let cancel = AtomicBool::new(false);
    let nominal = travel();
    let tiny_speed = Travel {
        speed_mm_s: f64::MIN_POSITIVE,
        ..nominal
    };
    let overflowing_lift = Travel {
        lift_mm: f64::MAX,
        ..nominal
    };
    for (start, model, last_seam, stopped) in [
        (Some([f64::NAN, 0., 0.]), nominal, [0.; 3], false),
        (Some([f64::MAX, 0., 0.]), nominal, [0.; 3], false),
        (None, tiny_speed, [0.; 3], false),
        (None, overflowing_lift, [0.; 3], false),
        (None, nominal, [f64::INFINITY, 0., 0.], false),
        (None, nominal, [0., 1e101, 0.], false),
        (None, nominal, [0.; 3], true),
    ] {
        let seams = [[0.; 3], last_seam];
        let mut tasks = vec![open([0.; 3], [1.; 3], true); size];
        tasks[size - 1] = task(Ends::Loop { seams: &seams });
        let mut order = Order::new(
            &tasks,
            &model,
            &members,
            start,
            members.iter().map(|&local| (local, false, 0)).collect(),
            vec![vec![]; size],
            vec![vec![]; size],
        );
        cancel.store(stopped, Ordering::Relaxed);
        assert!(!order.enable_simd(&cancel));
        assert!(order.simd.is_none());
    }
}
