// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

fn path(x: f64) -> DepositionPath {
    DepositionPath {
        material_domain: None,
        kind: DepositionKind::Support,
        owner: None,
        width: WidthProfile::Constant { width_mm: 0.4 },
        physical_z_mm: [0.0, 0.2],
        points_mm: vec![[x, 0.5, 0.2], [x + 1.0, 0.5, 0.2]],
        arcs: Vec::new(),
        exposed: false,
        connectors: Vec::new(),
    }
}

fn layer(paths: Vec<DepositionPath>) -> PrintLayer {
    PrintLayer {
        index: 17,
        hatch_course: 0,
        z_mm: 0.2,
        lower_z_mm: 0.0,
        height_mm: 0.2,
        section_z_mm: 0.1,
        paths,
        skin_command_fits: Vec::new(),
        region_coverage: PlanarRegionCoverage {
            section_area_mm2: 0.0,
            source_section_area_mm2: 0.0,
            surface_added_area_mm2: 0.0,
            wall_area_mm2: 0.0,
            bottom_skin_area_mm2: 0.0,
            top_skin_area_mm2: 0.0,
            solid_interior_area_mm2: 0.0,
            sparse_interior_area_mm2: 0.0,
            intentional_void_area_mm2: 0.0,
            protected_hole_area_mm2: 0.0,
            nominal_deposition_volume_mm3: 0.0,
            support_area_mm2: 0.0,
            support_nominal_deposition_volume_mm3: 0.0,
            unsupported_overhang_mm: 0.0,
            support_omitted_wall_mm: 0.0,
            thin_widened_mm: 0.0,
            thin_omitted_area_mm2: 0.0,
            process_regions: Vec::new(),
            process_interfaces: Vec::new(),
            separator_area_mm2: 0.0,
            process_diaphragms: Vec::new(),
            diaphragm_area_mm2: 0.0,
            skin_bridge_domains: Vec::new(),
            web_roof_domains: Vec::new(),
        },
    }
}

fn evaluate(
    layers: &[PrintLayer],
    height: spiling_geometry::ProtectedVoidHeightScope,
    allowance: usize,
) -> (Result<(), ManufacturingError>, ClearanceTrace) {
    let cancel = AtomicBool::new(false);
    let mut work = 0;
    let index = clearance_index::ClearanceIndex::build(
        layers.iter().map(|layer| layer.paths.as_slice()),
        allowance,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert_eq!(index.is_some(), allowance != 0);
    let polygon =
        geo::Rect::new(geo::Coord { x: 0.0, y: 0.0 }, geo::Coord { x: 1.0, y: 1.0 }).to_polygon();
    let trace = ClearanceTrace {
        enabled: true,
        ..ClearanceTrace::default()
    };
    let result = certify_polygon_capsules(
        PolygonDeposition {
            layers,
            range: 0..layers.len(),
            index: index.as_ref(),
        },
        &ForbiddenPolygonScope {
            polygon: &polygon,
            height,
            label: "ordered-index regression",
        },
        0.2,
        &mut spiling_geometry::GeometryWorkBudget::new(1_000_000),
        &mut work,
        &trace,
        None,
        &cancel,
    );
    (result, trace)
}

#[test]
fn indexed_native_clearance_matches_exhaustive_first_error_and_narrow_tests() {
    for variant in 0..9 {
        let mut paths = (0..65).map(|_| path(100.0)).collect::<Vec<_>>();
        let mut height = spiling_geometry::ProtectedVoidHeightScope::Bounded {
            lower_mm: 0.0,
            upper_mm: 0.2,
        };
        match variant {
            0 => {}
            1 => paths[0] = path(0.0),
            2 => {
                paths[0] = path(0.0);
                paths[64].width = WidthProfile::Constant { width_mm: f64::NAN };
            }
            3 => {
                paths[0].width = WidthProfile::Constant { width_mm: f64::NAN };
                paths[64] = path(0.0);
            }
            4 => {
                paths[64].points_mm = vec![[-2.0, 1.35, 0.2], [0.0, 1.35, 0.2], [2.0, 1.35, 0.2]];
                paths[64].width = WidthProfile::PerEdge {
                    widths_mm: vec![0.25, 1.0],
                };
            }
            5 => {
                paths[0] = path(0.0);
                height = spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm: -0.1 };
            }
            6 => {
                paths[64] = path(0.0);
                paths[64].kind = DepositionKind::Brim;
            }
            7 => {
                paths[64] = path(0.0);
                paths[64].kind = DepositionKind::Skirt;
            }
            _ => {
                paths[0] = path(0.0);
                paths[0].kind = DepositionKind::Perimeter;
            }
        }
        let layers = [layer(paths)];
        let (ordinary, ordinary_trace) = evaluate(&layers, height, 0);
        let (indexed, indexed_trace) = evaluate(&layers, height, usize::MAX);
        assert_eq!(
            format!("{indexed:?}"),
            format!("{ordinary:?}"),
            "variant {variant}"
        );
        assert_eq!(
            indexed_trace.polygon_tests.get(),
            ordinary_trace.polygon_tests.get()
        );
        if variant == 0 {
            assert!(indexed.is_ok());
            assert_eq!(indexed_trace.skipped_paths.get(), 65);
            assert_eq!(indexed_trace.paths.get(), 0);
        }
        if [1, 2, 4, 6, 7].contains(&variant) {
            assert_eq!(
                indexed.as_ref().unwrap_err().code,
                ManufacturingErrorCode::NativeAnalysisIndeterminate
            );
        }
        if variant == 3 {
            assert_eq!(
                indexed.as_ref().unwrap_err().code,
                ManufacturingErrorCode::InvalidSpecification
            );
        }
    }
}

#[test]
fn deposited_blocks_preserve_ordered_errors_and_narrow_proofs() {
    let cancel = AtomicBool::new(false);
    let sweep = withdrawal_test_sweep(
        0.625,
        &mut spiling_geometry::GeometryWorkBudget::compile(),
        &cancel,
    );
    let sweep_bytes = sweep.retained_bytes();
    assert_ne!(sweep_bytes, 0);
    let schedule = [LayerElevation {
        lower_z: 0.0,
        z: 0.2,
        sample: 0.1,
        height: 0.2,
        course: 0,
    }];
    let mut distant = path(100.0);
    distant.kind = DepositionKind::Perimeter;
    let mut collision = path(0.0);
    collision.kind = DepositionKind::Bridge;
    let mut malformed = distant.clone();
    malformed.width = WidthProfile::PerEdge {
        widths_mm: Vec::new(),
    };
    let mut brim = collision.clone();
    brim.kind = DepositionKind::Brim;
    for (tail, expected, narrow_tests) in [
        (Vec::new(), None, 0),
        (
            vec![collision.clone(), malformed.clone()],
            Some(ManufacturingErrorCode::NativeAnalysisIndeterminate),
            1,
        ),
        (
            vec![malformed, collision],
            Some(ManufacturingErrorCode::InvalidSpecification),
            0,
        ),
        (
            vec![brim],
            Some(ManufacturingErrorCode::NativeAnalysisIndeterminate),
            1,
        ),
    ] {
        let mut paths = vec![path(0.0)];
        paths.extend(std::iter::repeat_n(distant.clone(), BLOCK_PATHS * 2 - 1));
        paths.extend(tail);
        let layers = [layer(paths)];
        let mut scan_work = 0;
        let deposition =
            deposition_vertical_extensions(&layers, &schedule, &mut scan_work, &cancel)
                .unwrap()
                .unwrap();
        let build = |bytes, work: &mut usize, cancel: &AtomicBool| {
            prepare_deposited_screen(&layers, &deposition, bytes, sweep_bytes, work, cancel)
        };
        let index = build(usize::MAX, &mut scan_work, &cancel).unwrap().unwrap();
        let exact = index.retained_bytes() + sweep_bytes;
        assert!(build(exact - 1, &mut scan_work, &cancel).unwrap().is_none());
        assert!(build(exact, &mut scan_work, &cancel).unwrap().is_some());
        assert_eq!(
            build(usize::MAX, &mut scan_work, &AtomicBool::new(true))
                .err()
                .unwrap()
                .code,
            ManufacturingErrorCode::Cancelled
        );
        let run = |screen, mut work| {
            let trace = ClearanceTrace {
                enabled: true,
                ..Default::default()
            };
            let result = certify_deposited_clearance(
                &ActualDepositionScope {
                    layers: &layers,
                    schedule: &schedule,
                    vertical: deposition.vertical,
                    screen,
                },
                0.2,
                0.1,
                &mut spiling_geometry::GeometryWorkBudget::new(1_000_000),
                &mut work,
                &trace,
                &cancel,
            );
            (result.err(), trace)
        };
        let (exhaustive, exhaustive_trace) = run(None, 0);
        let (screened, screened_trace) = run(Some(&index), scan_work);
        assert_eq!(exhaustive.as_ref().map(|error| error.code), expected);
        assert_eq!(format!("{screened:?}"), format!("{exhaustive:?}"));
        assert_eq!(exhaustive_trace.pair_tests.get(), narrow_tests);
        assert_eq!(screened_trace.pair_tests.get(), narrow_tests);
        assert!(screened_trace.deposited_block_skipped_slots.get() >= BLOCK_PATHS);
    }
}

#[test]
fn near_work_overflow_leaves_the_first_collision_in_authority() {
    let cancel = AtomicBool::new(false);
    let schedule = [LayerElevation {
        lower_z: 0.0,
        z: 0.2,
        sample: 0.1,
        height: 0.2,
        course: 0,
    }];
    let mut collision = path(0.0);
    collision.kind = DepositionKind::Bridge;
    let mut distant = path(100.0);
    distant.kind = DepositionKind::Perimeter;
    let mut paths = vec![path(0.0), collision];
    paths.extend(std::iter::repeat_n(distant, 64));
    let layers = [layer(paths)];
    let deposition = deposition_vertical_extensions(&layers, &schedule, &mut 0, &cancel)
        .unwrap()
        .unwrap();
    let before = usize::MAX - 64;
    let mut work = before;
    let screen =
        prepare_deposited_screen(&layers, &deposition, usize::MAX, 0, &mut work, &cancel).unwrap();
    assert!(screen.is_none());
    assert_eq!(work, before);
    let failure = certify_deposited_clearance(
        &ActualDepositionScope {
            layers: &layers,
            schedule: &schedule,
            vertical: deposition.vertical,
            screen: screen.as_ref(),
        },
        0.2,
        0.1,
        &mut spiling_geometry::GeometryWorkBudget::new(1024),
        &mut work,
        &ClearanceTrace::default(),
        &cancel,
    )
    .unwrap_err();
    assert_eq!(
        failure.code,
        ManufacturingErrorCode::NativeAnalysisIndeterminate
    );
}

#[test]
fn deposited_screen_keeps_widest_last_edge_at_threshold_in_both_z_directions() {
    let cancel = AtomicBool::new(false);
    let mut schedule = [0.0, 0.2].map(|lower_z| LayerElevation {
        lower_z,
        z: lower_z + 0.2,
        sample: lower_z + 0.1,
        height: 0.2,
        course: 0,
    });
    schedule[1].course = 1;
    let xy_gap = 0.2;
    let threshold = 0.5 + (0.4 + 0.6) * 0.5 + 2.0 * XYZ6_SCREEN_SLACK_MM + xy_gap;
    for support_layer in 0..schedule.len() {
        for ordinate in [threshold.next_down(), threshold, threshold.next_up()] {
            let object_z = schedule[1 - support_layer].z;
            let mut object = path(0.0);
            object.kind = DepositionKind::Bridge;
            object.physical_z_mm = [schedule[1 - support_layer].lower_z, object_z];
            object.width = WidthProfile::PerEdge {
                widths_mm: vec![0.25, 0.6],
            };
            object.points_mm = vec![
                [-1.0, ordinate, object_z],
                [0.0, ordinate, object_z],
                [1.0, ordinate, object_z + 0.025],
            ];
            let mut support = path(0.0);
            support.physical_z_mm = [schedule[support_layer].lower_z, schedule[support_layer].z];
            for point in &mut support.points_mm {
                point[2] = schedule[support_layer].z;
            }
            let mut layers = [layer(vec![object]), layer(Vec::new())];
            if support_layer == 0 {
                layers.swap(0, 1);
            }
            layers[support_layer].paths.push(support);
            for (ordinal, current) in layers.iter_mut().enumerate() {
                current.index = ordinal as u32;
                current.lower_z_mm = schedule[ordinal].lower_z;
                current.z_mm = schedule[ordinal].z;
            }
            let deposition = deposition_vertical_extensions(&layers, &schedule, &mut 0, &cancel)
                .unwrap()
                .unwrap();
            let index =
                prepare_deposited_screen(&layers, &deposition, usize::MAX, 0, &mut 0, &cancel)
                    .unwrap()
                    .unwrap();
            let evaluate = |screen| {
                let trace = ClearanceTrace {
                    enabled: true,
                    ..Default::default()
                };
                let result = certify_deposited_clearance(
                    &ActualDepositionScope {
                        layers: &layers,
                        schedule: &schedule,
                        vertical: deposition.vertical,
                        screen,
                    },
                    xy_gap,
                    0.2,
                    &mut spiling_geometry::GeometryWorkBudget::compile(),
                    &mut 0,
                    &trace,
                    &cancel,
                );
                (format!("{result:?}"), trace.pair_tests.get())
            };
            assert_eq!(evaluate(Some(&index)), evaluate(None));
        }
    }
}

#[test]
fn cancellation_inside_deposited_index_build_is_fatal() {
    use std::sync::atomic::Ordering;

    let cancel = AtomicBool::new(false);
    let mut object = path(10.0);
    object.kind = DepositionKind::Perimeter;
    let layers = [layer(vec![object; 32])];
    let mut work = 0;
    let error = ObjectClearanceIndex::build(
        &layers,
        usize::MAX,
        |kind| !kind.is_support(),
        || {
            if work == 8 {
                cancel.store(true, Ordering::Relaxed);
            }
            charge_support_clearance(&mut work, &cancel)
        },
    )
    .err()
    .unwrap();
    assert_eq!(work, 8);
    assert_eq!(error.code, ManufacturingErrorCode::Cancelled);
}

#[test]
fn pocket_relief_requires_the_full_actual_bead_to_clear_lower_geometry() {
    let cancel = AtomicBool::new(false);
    let schedule = [
        LayerElevation {
            lower_z: 0.0,
            z: 0.2,
            sample: 0.1,
            height: 0.2,
            course: 0,
        },
        LayerElevation {
            lower_z: 0.2,
            z: 0.4,
            sample: 0.3,
            height: 0.2,
            course: 1,
        },
        LayerElevation {
            lower_z: 0.4,
            z: 0.6,
            sample: 0.5,
            height: 0.2,
            course: 2,
        },
    ];
    let empty = MultiPolygon(Vec::new());
    let obstacle = |lower_y: f64| {
        MultiPolygon(vec![
            geo::Rect::new([0.0, lower_y], [2.0, lower_y + 1.0]).to_polygon(),
        ])
    };
    let protected = MultiPolygon(vec![geo::Rect::new([0.0, 0.0], [2.0, 2.0]).to_polygon()]);
    let mut support = path(0.5);
    support.physical_z_mm = [0.4, 0.6];
    for point in &mut support.points_mm {
        point[2] = 0.6;
    }
    for (lease, source_lip, deposited_lip, expected) in [
        (None, false, false, Some("projected protected native void")),
        (
            Some(1),
            false,
            false,
            Some("projected protected native void"),
        ),
        (Some(2), false, false, None),
        (
            Some(2),
            true,
            false,
            Some("support withdrawal: actual bead against continuous"),
        ),
        (
            Some(2),
            false,
            true,
            Some("support withdrawal: actual bead against deposited"),
        ),
    ] {
        let sections: Vec<_> = (0..3)
            .map(|index| crate::planner::SectionSpan {
                start: index,
                end: index + 1,
                material: Arc::new(if index == 0 {
                    obstacle(if source_lip { 0.65 } else { 0.95 })
                } else {
                    empty.clone()
                }),
                process_domains: Vec::new(),
                occupied: None,
                source_area_mm2: 0.0,
            })
            .collect();
        let mut object = path(0.5);
        object.kind = DepositionKind::Perimeter;
        for point in &mut object.points_mm {
            point[1] = if deposited_lip { 0.75 } else { 1.15 };
        }
        let mut layers = [
            layer(vec![object]),
            layer(Vec::new()),
            layer(vec![support.clone()]),
        ];
        for (index, (layer, elevation)) in layers.iter_mut().zip(&schedule).enumerate() {
            layer.index = index as u32;
            layer.lower_z_mm = elevation.lower_z;
            layer.z_mm = elevation.z;
            layer.height_mm = elevation.height;
        }
        let policy = SupportPolicy::Breakaway {
            density: 0.3,
            xy_clearance_mm: 0.2,
            z_gap_mm: 0.1,
            interface_thickness_mm: 0.4,
        };
        let mut work = spiling_geometry::GeometryWorkBudget::compile();
        let mut scope = crate::planner::support::VoidScope::new(
            spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm: 1.0 },
            Some(Arc::new(spiling_geometry::tolerant::TolerantVoidRoof {
                height_mm: [0.9, 1.0],
                facets: Vec::new(),
            })),
        );
        if let Some(ordinal) = lease {
            scope.allow_test_bead(
                ordinal,
                MultiPolygon(vec![geo::Rect::new([-1.0, 0.0], [3.0, 2.0]).to_polygon()]),
            );
        }
        let result = certify_support_clearance(
            &layers,
            &SupportClearanceGeometry {
                sections: &sections,
                schedule: &schedule,
                protected: &protected,
                scopes: &[scope],
                index_bytes: 64 * 1024,
            },
            &policy,
            &mut work,
            &mut 0,
            &cancel,
        )
        .and_then(|sweep| {
            if let Some(sweep) = sweep {
                sweep.certify_realized(&layers, &policy, &mut work, &cancel)?;
            }
            Ok(())
        });
        if let Some(message) = expected {
            let failure = result.unwrap_err();
            assert!(failure.message.contains(message), "{failure:?}");
        } else {
            result.unwrap();
        }
    }
}

fn withdrawal_test_sweep(
    upper: f64,
    work: &mut spiling_geometry::GeometryWorkBudget,
    cancel: &AtomicBool,
) -> withdrawal::Sweep {
    let mut sweep = withdrawal::Sweep::new(64 * 1024);
    sweep
        .record(
            [[0.5, 0.5], [1.5, 0.5]],
            spiling_geometry::PlanarCapsuleClearancePolicy {
                radius_mm: 0.2,
                required_gap_mm: 0.2,
                endpoint_error_mm: 0.0,
            },
            upper,
            work,
            cancel,
        )
        .unwrap();
    sweep
        .certify_source(&[], &[], 0.2, work, &mut 0, cancel)
        .unwrap()
        .unwrap()
}

#[test]
fn withdrawal_accepts_an_exact_contact_gap() {
    let cancel = AtomicBool::new(false);
    let mut work = spiling_geometry::GeometryWorkBudget::compile();
    let sweep = withdrawal_test_sweep(0.625, &mut work, &cancel);
    let mut roof = path(0.5);
    roof.kind = DepositionKind::Perimeter;
    roof.physical_z_mm = [0.75, 0.875];
    for point in &mut roof.points_mm {
        point[2] = 0.875;
    }
    let layers = [layer(vec![roof])];
    for (z_gap_mm, clears) in [(0.125, true), (0.126, false)] {
        let policy = SupportPolicy::Breakaway {
            density: 0.3,
            xy_clearance_mm: 0.2,
            z_gap_mm,
            interface_thickness_mm: 0.4,
        };
        assert_eq!(
            sweep
                .certify_realized(&layers, &policy, &mut work, &cancel)
                .is_ok(),
            clears
        );
    }
}

#[test]
fn withdrawal_checks_retained_arc_beyond_clear_planner_chord() {
    let cancel = AtomicBool::new(false);
    let mut work = spiling_geometry::GeometryWorkBudget::compile();
    let sweep = withdrawal_test_sweep(0.6, &mut work, &cancel);
    let policy = SupportPolicy::Breakaway {
        density: 0.3,
        xy_clearance_mm: 0.2,
        z_gap_mm: 0.1,
        interface_thickness_mm: 0.4,
    };
    let mut object = path(0.95);
    object.kind = DepositionKind::Perimeter;
    object.points_mm = vec![[0.95, 1.104, 0.2], [1.05, 1.104, 0.2]];
    sweep
        .certify_realized(&[layer(vec![object.clone()])], &policy, &mut work, &cancel)
        .unwrap();

    object.arcs.push(spiling_contracts::manufacturing::ArcRun {
        first: 0,
        last: 1,
        center_mm: [1.0, 1.404],
        clockwise: false,
    });
    let failure = sweep
        .certify_realized(&[layer(vec![object])], &policy, &mut work, &cancel)
        .unwrap_err();
    assert_eq!(
        failure.code,
        ManufacturingErrorCode::NativeAnalysisIndeterminate
    );
    assert!(failure.message.contains("against deposited object"));
}

#[test]
fn physical_scope_ranges_preserve_irregular_height_endpoints_and_open_below() {
    let schedule = [(0.0, 0.123), (0.123, 0.347), (0.347, 0.51)]
        .into_iter()
        .enumerate()
        .map(|(course, (lower_z, z))| LayerElevation {
            lower_z,
            z,
            sample: (lower_z + z) * 0.5,
            height: z - lower_z,
            course: course as u32,
        })
        .collect::<Vec<_>>();
    let layers = (0..schedule.len())
        .map(|_| layer(vec![path(100.0)]))
        .collect::<Vec<_>>();
    let extensions = [0.002, 0.003];
    let cancel = AtomicBool::new(false);
    for boundary in [0.0_f64, 0.123, 0.347, 0.51] {
        for height in [boundary.next_down(), boundary, boundary.next_up()] {
            for lower in [None, Some(height - 0.01), Some(height)] {
                let scope = lower.map_or(
                    spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm: height },
                    |lower_mm| spiling_geometry::ProtectedVoidHeightScope::Bounded {
                        lower_mm,
                        upper_mm: height,
                    },
                );
                let expected = schedule
                    .iter()
                    .enumerate()
                    .filter(|(_, layer)| {
                        lower.is_none_or(|lower| (layer.z + extensions[1]).next_up() >= lower)
                            && (layer.lower_z - extensions[0]).next_down() <= height
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                let range =
                    scoped_support_layer_range(&schedule, scope, extensions, &mut 0, &cancel)
                        .unwrap();
                assert_eq!(range.clone().collect::<Vec<_>>(), expected);
                let borrowed =
                    scoped_support_layers(&layers, &schedule, scope, extensions, &mut 0, &cancel)
                        .unwrap();
                assert_eq!(borrowed.as_ptr(), layers[range].as_ptr());
            }
        }
    }
}

#[test]
fn skipped_range_work_preserves_the_original_overflow_and_cancellation_state() {
    let cancel = AtomicBool::new(false);
    for remaining in [0, 1, 3, 16] {
        let mut scalar = usize::MAX - remaining;
        let mut batched = scalar;
        let ordinary = (0..17).try_for_each(|_| charge_support_clearance(&mut scalar, &cancel));
        let indexed = charge_support_clearance_range(&mut batched, 17, &cancel);
        assert_eq!(format!("{ordinary:?}"), format!("{indexed:?}"));
        assert_eq!(scalar, batched);
    }
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut work = 7;
    assert_eq!(
        charge_support_clearance_range(&mut work, 17, &cancel)
            .unwrap_err()
            .code,
        ManufacturingErrorCode::Cancelled
    );
    assert_eq!(work, 7);
}
