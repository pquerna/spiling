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
    let (result, trace, _) = evaluate_with_budget(
        layers,
        height,
        allowance,
        &mut spiling_geometry::GeometryWorkBudget::new(1_000_000),
    );
    (result, trace)
}

fn evaluate_with_budget(
    layers: &[PrintLayer],
    height: spiling_geometry::ProtectedVoidHeightScope,
    allowance: usize,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
) -> (Result<(), ManufacturingError>, ClearanceTrace, usize) {
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
        native_work,
        &mut work,
        &trace,
        None,
        &cancel,
    );
    (result, trace, work)
}

fn repeated_emitted_chord() -> [DepositionPath; 2] {
    [0.0000001, 0.0000002].map(|drift| {
        let mut bead = path(1.5);
        bead.points_mm = vec![[1.500001 + drift, 3.0, 0.2], [4.000001 + drift, 3.0, 0.2]];
        assert_eq!(
            emitted_xy_segment(&bead.points_mm).unwrap().0,
            [[1.500001, 3.0], [4.000001, 3.0]],
        );
        bead
    })
}

fn protected_ring() -> geo::Polygon<f64> {
    geo::Polygon::new(
        geo::Rect::new([0.0, 0.0], [6.0, 6.0])
            .to_polygon()
            .into_inner()
            .0,
        vec![
            geo::Rect::new([1.0, 1.0], [5.0, 5.0])
                .to_polygon()
                .into_inner()
                .0,
        ],
    )
}

const VOID_TEST_POLICY: SupportPolicy = SupportPolicy::Breakaway {
    density: 0.3,
    xy_clearance_mm: 0.2,
    z_gap_mm: 0.1,
    interface_thickness_mm: 0.4,
};

const VOID_TEST_SCHEDULE: [LayerElevation; 1] = [LayerElevation {
    lower_z: 0.0,
    z: 0.2,
    sample: 0.1,
    height: 0.2,
    course: 0,
}];

const VOID_TEST_HEIGHT: spiling_geometry::ProtectedVoidHeightScope =
    spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm: 1.0 };

#[test]
fn escaped_void_proofs_keep_the_first_width_and_mask_refusals() {
    let repeated = repeated_emitted_chord();
    let mut wider = repeated[0].clone();
    wider.width = WidthProfile::Constant { width_mm: 0.8 };
    let width_layers = [layer(repeated.iter().cloned().chain([wider]).collect())];
    let mask_layers = [layer(repeated.to_vec())];
    let ring = protected_ring();
    let filled = geo::Polygon::new(ring.exterior().clone(), Vec::new());
    let cancel = AtomicBool::new(false);
    let kept = crate::planner::support::KeptSupport::default();
    // The wider chord's escape box reaches lowest in X, so it is proved
    // first; the mask case proves both chords clear of the ring first.
    for (layers, protected, expected_width, tests) in [
        (&width_layers, MultiPolygon(vec![ring.clone()]), "0.8", 1),
        (&mask_layers, MultiPolygon(vec![ring, filled]), "0.4", 3),
    ] {
        let scopes = protected
            .0
            .iter()
            .map(|_| crate::planner::support::VoidScope::new(VOID_TEST_HEIGHT, None))
            .collect::<Vec<_>>();
        let geometry = SupportClearanceGeometry {
            sections: &[],
            schedule: &VOID_TEST_SCHEDULE,
            protected: &protected,
            scopes: &scopes,
            index_bytes: 128 * 1024,
            kept: &kept,
        };
        let run = || {
            let mut work = spiling_geometry::GeometryWorkBudget::compile();
            let mut support_work = 0;
            let trace = ClearanceTrace {
                enabled: true,
                ..ClearanceTrace::default()
            };
            let failure = certify_support_clearance_traced(
                layers,
                &geometry,
                &VOID_TEST_POLICY,
                &mut work,
                &mut support_work,
                &trace,
                &cancel,
            )
            .err()
            .expect("first protected clearance refusal");
            assert_eq!(trace.escaped_edges.get(), layers[0].paths.len());
            (
                failure,
                work.consumed(),
                support_work,
                trace.void_tests.get(),
            )
        };
        let (failure, _, _, void_tests) = run();
        assert_eq!(
            failure.code,
            ManufacturingErrorCode::NativeAnalysisIndeterminate
        );
        assert!(failure.message.contains("projected protected native void"));
        assert!(
            failure
                .message
                .contains(&format!("width {expected_width} mm")),
            "{failure:?}"
        );
        assert_eq!(void_tests, tests);
        assert_eq!(run(), run());
    }
}

#[test]
fn relieved_voids_record_the_withdrawal_beside_escaped_exclusions() {
    let cancel = AtomicBool::new(false);
    let mut distant = path(8.0);
    distant
        .points_mm
        .iter_mut()
        .for_each(|point| point[1] = 3.0);
    let layers = [layer(
        repeated_emitted_chord()
            .into_iter()
            .chain([distant])
            .collect(),
    )];
    let first = geo::Rect::new([1.4, 2.5], [4.2, 3.5]).to_polygon();
    let last = geo::Rect::new([7.5, 2.5], [9.5, 3.5]).to_polygon();
    let protected = MultiPolygon(vec![first.clone(), protected_ring(), last.clone()]);
    let lease = |polygon| {
        let mut scope = crate::planner::support::VoidScope::new(
            VOID_TEST_HEIGHT,
            Some(Arc::new(spiling_geometry::tolerant::TolerantVoidRoof {
                height_mm: [0.9, 1.0],
                facets: Vec::new(),
            })),
        );
        scope.allow_test_bead(0, MultiPolygon(vec![polygon]));
        scope
    };
    let kept = crate::planner::support::KeptSupport::default();
    for last_leased in [true, false] {
        let scopes = [
            lease(first.clone()),
            crate::planner::support::VoidScope::new(VOID_TEST_HEIGHT, None),
            if last_leased {
                lease(last.clone())
            } else {
                crate::planner::support::VoidScope::new(VOID_TEST_HEIGHT, None)
            },
        ];
        let run = |allowance| {
            let mut work = spiling_geometry::GeometryWorkBudget::compile();
            let trace = ClearanceTrace {
                enabled: true,
                ..ClearanceTrace::default()
            };
            let proof = certify_support_clearance_traced(
                &layers,
                &SupportClearanceGeometry {
                    sections: &[],
                    schedule: &VOID_TEST_SCHEDULE,
                    protected: &protected,
                    scopes: &scopes,
                    index_bytes: allowance,
                    kept: &kept,
                },
                &VOID_TEST_POLICY,
                &mut work,
                &mut 0,
                &trace,
                &cancel,
            );
            (proof, trace, work)
        };
        let (proof, trace, mut work) = run(128 * 1024);
        // A relieved void keeps its full ordered-index proof.
        assert_ne!(trace.index_bytes.get(), 0);
        assert_ne!(trace.polygon_tests.get(), 0);
        let outcome = proof.map(|sweep| {
            let sweep = sweep.expect("leased beads survive as a withdrawal sweep");
            sweep
                .certify_realized(&layers, &VOID_TEST_POLICY, &mut work, &cancel)
                .unwrap();
            for chord in [0, 2] {
                let mut object = layers[0].paths[chord].clone();
                object.kind = DepositionKind::Perimeter;
                let failure = sweep
                    .certify_realized(
                        &[layer(vec![object])],
                        &VOID_TEST_POLICY,
                        &mut work,
                        &cancel,
                    )
                    .unwrap_err();
                assert!(failure.message.contains("against deposited object"));
            }
        });
        assert_eq!(outcome.is_ok(), last_leased, "{outcome:?}");
        // Escape records are admitted against the same allowance.
        assert_eq!(
            run(0).0.err().map(|failure| failure.code),
            Some(ManufacturingErrorCode::ResourceLimit)
        );
    }
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

fn certify_objects(
    layers: &[PrintLayer],
    schedule: &[LayerElevation],
    sections: &[crate::planner::SectionSpan],
    kept: &crate::planner::support::KeptSupport,
    [xy_gap, z_gap]: [f64; 2],
) -> (Result<(), ManufacturingError>, ClearanceTrace) {
    let trace = ClearanceTrace {
        enabled: true,
        ..ClearanceTrace::default()
    };
    let result = certify_support_clearance_traced(
        layers,
        &SupportClearanceGeometry {
            sections,
            schedule,
            protected: &MultiPolygon(Vec::new()),
            scopes: &[],
            index_bytes: 1 << 20,
            kept,
        },
        &SupportPolicy::Breakaway {
            density: 0.3,
            xy_clearance_mm: xy_gap,
            z_gap_mm: z_gap,
            interface_thickness_mm: 0.4,
        },
        &mut spiling_geometry::GeometryWorkBudget::new(1_000_000),
        &mut 0,
        &trace,
        &AtomicBool::new(false),
    )
    .map(|_| ());
    (result, trace)
}

#[test]
fn escaped_object_pairs_keep_ordered_errors_and_narrow_proofs() {
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
        paths.extend(std::iter::repeat_n(distant.clone(), 31));
        paths.extend(tail);
        let layers = [layer(paths)];
        let (result, trace) = certify_objects(
            &layers,
            &schedule,
            &[],
            &crate::planner::support::KeptSupport::default(),
            [0.2, 0.1],
        );
        assert_eq!(result.err().map(|error| error.code), expected);
        assert_eq!(trace.pair_tests.get(), narrow_tests);
    }
}

/// Every non-model edge against every non-support edge, unscreened.
fn exhaustive_object_pairs(layers: &[PrintLayer], gaps: [f64; 2]) -> bool {
    let cancel = AtomicBool::new(false);
    let mut work = spiling_geometry::GeometryWorkBudget::compile();
    let paths = || layers.iter().flat_map(|layer| &layer.paths);
    for support in paths().filter(|path| !path.kind.is_model()) {
        for object in paths().filter(|path| !path.kind.is_support()) {
            for s in support.checked_edges().unwrap() {
                for o in object.checked_edges().unwrap() {
                    if certify_capsule_pair(
                        &emitted_edge(support, s).unwrap(),
                        &emitted_edge(object, o).unwrap(),
                        gaps,
                        &mut work,
                        &ClearanceTrace::default(),
                        &cancel,
                        String::new,
                    )
                    .is_err()
                    {
                        return false;
                    }
                }
            }
        }
    }
    true
}

#[test]
fn escaped_object_screens_keep_the_widest_last_edge_at_threshold_in_both_z_directions() {
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
    let mut outcomes = Vec::new();
    for support_layer in 0..schedule.len() {
        for ordinate in [
            threshold - 0.01,
            threshold.next_down(),
            threshold,
            threshold.next_up(),
        ] {
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
            let (result, _) = certify_objects(
                &layers,
                &schedule,
                &[],
                &crate::planner::support::KeptSupport::default(),
                [xy_gap, 0.2],
            );
            let exhaustive = exhaustive_object_pairs(&layers, [xy_gap, 0.2]);
            assert_eq!(
                result.is_ok(),
                exhaustive,
                "{support_layer} {ordinate}: {result:?}"
            );
            outcomes.push(exhaustive);
        }
    }
    assert!(outcomes.contains(&false));
}

#[test]
fn object_beads_the_sweep_never_excluded_are_proved_directly() {
    let schedule = [0.0, 0.2].map(|lower_z| LayerElevation {
        lower_z,
        z: lower_z + 0.2,
        sample: lower_z + 0.1,
        height: 0.2,
        course: 0,
    });
    // An object bead poking outside its section material, and support one
    // layer up whose bead is 0.1 mm short of the XY gap from it.
    let mut object = path(1.0);
    object.kind = DepositionKind::Perimeter;
    object.points_mm = vec![[1.0, 4.15, 0.2], [3.0, 4.15, 0.2]];
    let mut support = path(1.0);
    support.physical_z_mm = [0.2, 0.4];
    support.points_mm = vec![[1.0, 4.65, 0.4], [3.0, 4.65, 0.4]];
    let mut layers = [layer(vec![object]), layer(vec![support])];
    for (ordinal, current) in layers.iter_mut().enumerate() {
        current.index = ordinal as u32;
        current.lower_z_mm = schedule[ordinal].lower_z;
        current.z_mm = schedule[ordinal].z;
    }
    let sections: Vec<_> = (0..2)
        .map(|index| crate::planner::SectionSpan {
            start: index,
            end: index + 1,
            material: Arc::new(MultiPolygon(if index == 0 {
                vec![geo::Rect::new([0.0, 0.0], [4.0, 4.0]).to_polygon()]
            } else {
                Vec::new()
            })),
            process_domains: Vec::new(),
            occupied: None,
            source_area_mm2: 0.0,
        })
        .collect();
    let everywhere = MultiPolygon(vec![geo::Rect::new([-2.0, -2.0], [8.0, 8.0]).to_polygon()]);
    let kept = || {
        crate::planner::support::KeptSupport::for_test(
            [[-2.0, -2.0], [8.0, 8.0]],
            [(1, [-1.0, 1.0], everywhere.clone())],
        )
    };
    let edge = ([1.0, 4.15], [3.0, 4.15], 0.2);
    let other = ([1.0, 0.2], [3.0, 0.2], 0.2);
    for (record, unseen, accepted) in [
        // The sweep excluded exactly this bead: its kept cells vouch for it.
        (Some(([-1.0, 1.0], edge)), 0, true),
        // It excluded another bead: this one is proved and refused.
        (Some(([-1.0, 1.0], other)), 1, false),
        // The object band left the band the scope was applied for.
        (Some(([-1.0, 0.1], edge)), 1, false),
        // No scope at all.
        (None, 1, false),
    ] {
        let kept = match record {
            Some((band, edge)) => kept().with_test_scope(0, band, &[edge]),
            None => kept(),
        };
        let (result, trace) = certify_objects(&layers, &schedule, &sections, &kept, [0.2, 0.1]);
        assert_eq!(result.is_ok(), accepted, "{result:?}");
        assert_eq!(trace.escaped_edges.get(), 0);
        assert_eq!(trace.unseen_object_edges.get(), unseen);
        if !accepted {
            assert!(
                result
                    .unwrap_err()
                    .message
                    .contains("versus deposited object/bridge layer 0")
            );
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
    let error = crate::clearance_screen::ObjectClearanceIndex::build(
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
                kept: &crate::planner::support::KeptSupport::default(),
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

#[test]
fn escaped_occupancy_proves_only_nearby_pieces_and_keeps_the_first_refusal() {
    let schedule: Vec<_> = (0..12)
        .map(|index| {
            let lower_z = f64::from(index) * 0.2;
            LayerElevation {
                lower_z,
                z: lower_z + 0.2,
                sample: lower_z + 0.1,
                height: 0.2,
                course: index as u32,
            }
        })
        .collect();
    for refuses in [false, true] {
        // Many independent raw pieces per section; the refusing case puts
        // one piece under a support bead mid-scan.
        let sections: Vec<_> = (0..schedule.len() as u32)
            .map(|index| crate::planner::SectionSpan {
                start: index,
                end: index + 1,
                material: Arc::new(MultiPolygon(
                    (0..48)
                        .map(|piece| {
                            let x = 10.0 + f64::from(piece) * 3.0;
                            let y = if refuses && index == 7 && piece == 13 {
                                4.3
                            } else {
                                5.0
                            };
                            geo::Rect::new([x, y], [x + 1.0, y + 1.0]).to_polygon()
                        })
                        .collect(),
                )),
                process_domains: Vec::new(),
                occupied: None,
                source_area_mm2: 0.0,
            })
            .collect();
        let layers: Vec<_> = schedule
            .iter()
            .enumerate()
            .map(|(index, elevation)| {
                // A loop around the whole row, clear of every piece unless
                // moved.
                let mut support = path(0.0);
                support.physical_z_mm = [elevation.lower_z, elevation.z];
                support.points_mm = [[8.0, 4.4], [160.0, 4.4], [160.0, 6.6], [8.0, 6.6]]
                    .map(|[x, y]| [x, y, elevation.z])
                    .to_vec();
                let mut current = layer(vec![support]);
                current.index = index as u32;
                current.z_mm = elevation.z;
                current.lower_z_mm = elevation.lower_z;
                current.height_mm = elevation.height;
                current
            })
            .collect();
        let evaluate = || {
            let (result, trace) = certify_objects(
                &layers,
                &schedule,
                &sections,
                &crate::planner::support::KeptSupport::default(),
                [0.2, 0.1],
            );
            (
                result.err().map(|e| (e.code, e.message)),
                trace.escaped_edges.get(),
                trace.occupancy_tests.get(),
            )
        };
        let outcome = evaluate();
        assert_eq!(outcome.0.is_some(), refuses, "{outcome:?}");
        if let Some((code, message)) = &outcome.0 {
            assert_eq!(*code, ManufacturingErrorCode::NativeAnalysisIndeterminate);
            assert!(message.contains("against continuous native/finish outer occupancy"));
        }
        // Without kept cells every support edge escapes whole, yet only the
        // moved piece comes within the gap of an escape box.
        assert_eq!(outcome.1, 3 * schedule.len());
        assert_eq!(outcome.2 > 0, refuses);
        assert_eq!(evaluate(), outcome);
    }
}

/// One support bead on a layer whose kept cells are `[0, 2] × [0, 1]`,
/// ending at `end_x`, beside one occupied piece starting at `piece_x`.
fn kept_tie_break(piece_x: f64, end_x: f64) -> (Result<(), ManufacturingError>, ClearanceTrace) {
    let schedule = [LayerElevation {
        lower_z: 0.0,
        z: 0.2,
        sample: 0.1,
        height: 0.2,
        course: 0,
    }];
    let mut support = path(0.5);
    support.points_mm = vec![[0.5, 0.5, 0.2], [end_x, 0.5, 0.2]];
    let mut current = layer(vec![support]);
    current.index = 0;
    let sections = [crate::planner::SectionSpan {
        start: 0,
        end: 1,
        material: Arc::new(MultiPolygon(vec![
            geo::Rect::new([piece_x, 0.0], [piece_x + 1.0, 1.0]).to_polygon(),
        ])),
        process_domains: Vec::new(),
        occupied: None,
        source_area_mm2: 0.0,
    }];
    let kept = crate::planner::support::KeptSupport::for_test(
        [[-1.0, -1.0], [5.0, 2.0]],
        [(
            0,
            [-1.0, 1.0],
            MultiPolygon(vec![geo::Rect::new([0.0, 0.0], [2.0, 1.0]).to_polygon()]),
        )],
    );
    certify_objects(&[current], &schedule, &sections, &kept, [0.2, 0.1])
}

#[test]
fn kept_cells_certify_beads_and_only_escaped_cells_take_an_exact_proof() {
    // Inside the kept cells: certified with no exact proof, although the
    // piece's box is within a millimetre.
    let (result, trace) = kept_tie_break(2.3, 1.5);
    result.unwrap();
    assert_eq!(trace.support_edges.get(), 1);
    assert_eq!(trace.escaped_edges.get(), 0);
    assert_eq!(trace.occupancy_tests.get(), 0);

    // The bead's end leaves the kept cells but stays 0.02 mm beyond the XY
    // gap from the piece: the local exact tie-break passes.
    let (result, trace) = kept_tie_break(2.27, 1.85);
    result.unwrap();
    assert_eq!(trace.escaped_edges.get(), 1);
    assert!(trace.escaped_cells.get() > 0);
    assert_eq!(trace.occupancy_tests.get(), 1);

    // Truly too close: the same tie-break refuses.
    let (result, trace) = kept_tie_break(2.15, 1.85);
    let failure = result.unwrap_err();
    assert_eq!(
        failure.code,
        ManufacturingErrorCode::NativeAnalysisIndeterminate
    );
    assert!(
        failure
            .message
            .contains("against continuous native/finish outer occupancy")
    );
    assert_eq!(trace.escaped_edges.get(), 1);
    assert_eq!(trace.occupancy_tests.get(), 1);
}
