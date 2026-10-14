// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use classification::{ClassifiedRegions, WallLayout};
use geo::{Coord, LineString, Polygon};

pub(super) fn recipe(fraction: f64) -> PlanarPrintRecipe {
    PlanarPrintRecipe {
        id: "region-test".into(),
        revision: "1".into(),
        material_id: "nominal".into(),
        material_revision: "1".into(),
        filament_diameter_mm: 1.75,
        layer_height_mm: 0.4,
        bead_width_mm: 0.6,
        perimeter_count: 1,
        top_skin_thickness_mm: 0.81,
        bottom_skin_thickness_mm: 0.41,
        infill_fraction: fraction,
        print_speed_mm_s: 20.0,
        travel_speed_mm_s: 100.0,
        flow_multiplier: 1.0,
        process: ProcessPolicy {
            first_layer_speed_factor: 1.0,
            first_layer_flow_factor: 1.0,
            fan_first_layers: 1,
            fan_after_first_layers: 0,
            bridge_speed_factor: 0.5,
            bridge_flow_factor: 1.0,
            bridge_fan_pwm: 255,
            max_bridge_span_mm: 6.0,
            retraction: RetractionPolicy::Disabled,
        },
        supports: SupportPolicy::Disabled,
    }
}
fn ring(x: f64, y: f64, width: f64, height: f64) -> LineString<f64> {
    LineString(vec![
        Coord { x, y },
        Coord { x: x + width, y },
        Coord {
            x: x + width,
            y: y + height,
        },
        Coord { x, y: y + height },
        Coord { x, y },
    ])
}
fn rectangle(width: f64, height: f64) -> MultiPolygon<f64> {
    MultiPolygon(vec![Polygon::new(ring(0.0, 0.0, width, height), vec![])])
}
fn empty() -> MultiPolygon<f64> {
    MultiPolygon(vec![])
}
fn classify(
    section: &MultiPolygon<f64>,
    below: &MultiPolygon<f64>,
    above: &MultiPolygon<f64>,
    recipe: &PlanarPrintRecipe,
    cancel: &AtomicBool,
) -> Result<ClassifiedRegions, ManufacturingError> {
    let layout = Arc::new(WallLayout::new(
        section,
        recipe,
        region::point_count(section),
        cancel,
    )?);
    ClassifiedRegions::new(layout, below, above, recipe, cancel)
}
fn output(
    section: &MultiPolygon<f64>,
    below: &MultiPolygon<f64>,
    above: &MultiPolygon<f64>,
    fraction: f64,
    index: u32,
) -> (Vec<DepositionPath>, PlanarRegionCoverage) {
    let cancel = AtomicBool::new(false);
    let recipe = recipe(fraction);
    classify(section, below, above, &recipe, &cancel)
        .unwrap()
        .paths(
            0.4 * (index + 1) as f64,
            0.4,
            region::HatchAxis::preferred(index),
            &recipe,
            &mut 0,
            &cancel,
        )
        .unwrap()
}
fn partition(coverage: &PlanarRegionCoverage) {
    let sum = coverage.wall_area_mm2
        + coverage.bottom_skin_area_mm2
        + coverage.top_skin_area_mm2
        + coverage.solid_interior_area_mm2
        + coverage.sparse_interior_area_mm2;
    assert!(
        (sum - coverage.section_area_mm2).abs() < 1e-7,
        "partition residual {}: {coverage:?}",
        sum - coverage.section_area_mm2
    );
}

#[test]
fn native_cap_disappearing_offset_supports_preserve_material_and_bead_clearance() {
    use geo::{Contains, Point};
    let points: Vec<[f64; 2]> = serde_json::from_slice(include_bytes!(
        "../../../../fixtures/manufacturing/native/tilted-cap-offset.points.json"
    ))
    .unwrap();
    let source = Polygon::new(
        LineString(
            points
                .into_iter()
                .map(|p| Coord { x: p[0], y: p[1] })
                .collect(),
        ),
        Vec::new(),
    );
    let material = MultiPolygon(vec![source.clone()]);
    let radius = 0.3;
    let result = region::offset(&material, -radius, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        result.0.len(),
        1,
        "a disappearing support edge must not create material islands"
    );
    assert!(result.0[0].interiors().is_empty());
    for coordinate in &result.0[0].exterior().0 {
        assert!(source.contains(&Point(*coordinate)));
        let distance = source
            .exterior()
            .0
            .windows(2)
            .map(|edge| {
                let dx = edge[1].x - edge[0].x;
                let dy = edge[1].y - edge[0].y;
                let squared_length = dx * dx + dy * dy;
                let t = if squared_length == 0.0 {
                    0.0
                } else {
                    (((coordinate.x - edge[0].x) * dx + (coordinate.y - edge[0].y) * dy)
                        / squared_length)
                        .clamp(0.0, 1.0)
                };
                (coordinate.x - edge[0].x - t * dx).hypot(coordinate.y - edge[0].y - t * dy)
            })
            .fold(f64::INFINITY, f64::min);
        assert!(
            distance >= radius - 0.000001,
            "unsafe bead center clearance {distance}"
        );
        assert!(
            distance <= radius + OFFSET_TOLERANCE_MM,
            "offset error exceeds declared bound: {distance}"
        );
    }
}

#[test]
fn protected_holes_and_islands_have_no_duplicate_or_void_crossing_paths() {
    let mut section = MultiPolygon(vec![
        Polygon::new(ring(0.0, 0.0, 20.0, 12.0), vec![ring(7.0, 4.0, 6.0, 4.0)]),
        Polygon::new(ring(30.0, 0.0, 8.0, 8.0), vec![]),
    ]);
    region::canonicalize(&mut section);
    for index in 0..2 {
        let (paths, coverage) = output(&section, &section, &section, 0.25, index);
        partition(&coverage);
        // Independent fixture geometry: two outer rectangles minus one hole.
        assert!((coverage.section_area_mm2 - (20.0 * 12.0 + 8.0 * 8.0 - 6.0 * 4.0)).abs() < 1e-7);
        // Erosion leaves rectangular outers and rounds the expanded hole.
        // The exact rounded-hole area exceeds its sagitta-bounded polygon by
        // at most arc length * sagitta (plus the second-order disk term).
        let radius = recipe(0.25).bead_width_mm;
        let analytic_fill = (20.0 - 2.0 * radius) * (12.0 - 2.0 * radius)
            + (8.0 - 2.0 * radius).powi(2)
            - (6.0 * 4.0 + 2.0 * (6.0 + 4.0) * radius + std::f64::consts::PI * radius.powi(2));
        let arc_area_bound = std::f64::consts::TAU * radius * OFFSET_TOLERANCE_MM
            + std::f64::consts::PI * OFFSET_TOLERANCE_MM.powi(2);
        assert!((coverage.sparse_interior_area_mm2 - analytic_fill).abs() < arc_area_bound);
        assert!((coverage.protected_hole_area_mm2 - 24.0).abs() < 1e-7);
        assert!(coverage.intentional_void_area_mm2 > 0.0);
        assert!(
            (coverage.intentional_void_area_mm2 - coverage.sparse_interior_area_mm2 * 0.75).abs()
                < 1e-7
        );
        let mut segments = BTreeSet::new();
        for path in paths {
            for pair in path.points_mm.windows(2) {
                let mut endpoints = [pair[0].map(f64::to_bits), pair[1].map(f64::to_bits)];
                endpoints.sort();
                assert!(segments.insert(endpoints), "duplicate deposition segment");
                let x = (pair[0][0] + pair[1][0]) * 0.5;
                let y = (pair[0][1] + pair[1][1]) * 0.5;
                assert!(!(x > 7.0 && x < 13.0 && y > 4.0 && y < 8.0));
                assert!(!(x > 20.0 && x < 30.0));
            }
        }
    }
}

#[test]
fn sparse_roof_floor_and_overlap_are_solid_and_owned_once() {
    let section = rectangle(20.0, 10.0);
    let (floor, bottom) = output(&section, &empty(), &section, 0.2, 0);
    let (roof, top) = output(&section, &section, &empty(), 0.2, 3);
    let (middle, sparse) = output(&section, &section, &section, 0.2, 2);
    assert!(floor.iter().any(|p| p.kind == DepositionKind::BottomSkin));
    assert!(roof.iter().any(|p| p.kind == DepositionKind::TopSkin));
    assert!(middle.iter().any(|p| p.kind == DepositionKind::SparseFill));
    assert!(bottom.sparse_interior_area_mm2 == 0.0 && top.sparse_interior_area_mm2 == 0.0);
    assert!(sparse.sparse_interior_area_mm2 > 0.0);
    let (overlap_paths, overlap) = output(&section, &empty(), &empty(), 0.2, 0);
    assert!(
        !overlap_paths
            .iter()
            .any(|p| p.kind == DepositionKind::TopSkin)
    );
    assert_eq!(overlap.top_skin_area_mm2, 0.0);
    assert_eq!(overlap.bottom_skin_area_mm2, bottom.bottom_skin_area_mm2);
    for coverage in [bottom, top, sparse, overlap] {
        partition(&coverage);
    }
}

#[test]
fn physical_skin_horizon_tracks_shrinking_local_top_and_slab_transition() {
    let sections = vec![
        SectionSpan {
            start: 0,
            end: 6,
            material: rectangle(20.0, 10.0),
            source_area_mm2: 200.0,
        },
        SectionSpan {
            start: 6,
            end: 12,
            material: rectangle(10.0, 10.0),
            source_area_mm2: 100.0,
        },
    ];
    let recipe = recipe(0.3);
    let schedule = layer_schedule(0.0, 4.8, recipe.layer_height_mm, &[]).unwrap();
    assert_eq!(
        physical_neighbor(&schedule, 0, recipe.top_skin_thickness_mm, true),
        Some(3)
    );
    assert_eq!(
        physical_neighbor(&schedule, 2, recipe.bottom_skin_thickness_mm, false),
        Some(0)
    );
    let cancel = AtomicBool::new(false);
    let mut work = 0;
    for index in 0..12u32 {
        let current = span_at(&sections, index);
        let below = physical_neighbor(&schedule, index, recipe.bottom_skin_thickness_mm, false)
            .map(|i| span_at(&sections, i));
        let above = physical_neighbor(&schedule, index, recipe.top_skin_thickness_mm, true)
            .map(|i| span_at(&sections, i));
        let lower = survival(&sections, current, below, &mut work, &cancel).unwrap();
        let upper = survival(&sections, current, above, &mut work, &cancel).unwrap();
        let (_, coverage) = output(&sections[current].material, &lower, &upper, 0.3, index);
        partition(&coverage);
        if index < 2 {
            assert!(coverage.bottom_skin_area_mm2 > 0.0);
        } else {
            assert_eq!(coverage.bottom_skin_area_mm2, 0.0);
        }
        if (3..6).contains(&index) {
            assert!(coverage.top_skin_area_mm2 > 0.0);
            assert!(coverage.sparse_interior_area_mm2 > 0.0);
        } else if index >= 9 {
            assert!(coverage.top_skin_area_mm2 > 0.0);
            assert_eq!(coverage.sparse_interior_area_mm2, 0.0);
        } else {
            assert_eq!(coverage.top_skin_area_mm2, 0.0);
        }
    }
}

#[test]
fn narrow_attached_rib_is_rejected_instead_of_erased() {
    let mut section = MultiPolygon(vec![Polygon::new(
        LineString(vec![
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 10.0, y: 0.0 },
            Coord { x: 10.0, y: 4.8 },
            Coord { x: 15.0, y: 4.8 },
            Coord { x: 15.0, y: 5.2 },
            Coord { x: 10.0, y: 5.2 },
            Coord { x: 10.0, y: 10.0 },
            Coord { x: 0.0, y: 10.0 },
            Coord { x: 0.0, y: 0.0 },
        ]),
        vec![],
    )]);
    region::canonicalize(&mut section);
    let result = classify(
        &section,
        &section,
        &section,
        &recipe(0.25),
        &AtomicBool::new(false),
    );
    assert_eq!(
        result.err().unwrap().code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
}

#[test]
fn deterministic_accounting_uses_emitted_lengths_and_alternating_axes() {
    let section = rectangle(20.0, 10.0);
    let recipe = recipe(0.25);
    for index in 0..2 {
        let first = output(&section, &section, &section, 0.25, index);
        let second = output(&section, &section, &section, 0.25, index);
        assert_eq!(first, second);
        let h = recipe.layer_height_mm;
        let area = ((recipe.bead_width_mm - h) * h + std::f64::consts::PI * (h * 0.5).powi(2))
            * recipe.flow_multiplier;
        let emitted = first
            .0
            .iter()
            .flat_map(|p| p.points_mm.windows(2))
            .map(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]) * area)
            .sum::<f64>();
        assert!((emitted - first.1.nominal_deposition_volume_mm3).abs() < 1e-9);
        for path in first
            .0
            .iter()
            .filter(|p| p.kind == DepositionKind::SparseFill)
        {
            if index == 0 {
                assert_eq!(path.points_mm[0][1], path.points_mm[1][1]);
            } else {
                assert_eq!(path.points_mm[0][0], path.points_mm[1][0]);
            }
        }
    }
}

#[test]
fn cancellation_and_retained_segment_budget_precede_path_growth() {
    let section = rectangle(20.0, 10.0);
    let recipe = recipe(0.3);
    assert_eq!(
        classify(
            &section,
            &section,
            &section,
            &recipe,
            &AtomicBool::new(true)
        )
        .err()
        .unwrap()
        .code,
        ManufacturingErrorCode::Cancelled
    );
    let classified = classify(
        &section,
        &section,
        &section,
        &recipe,
        &AtomicBool::new(false),
    )
    .unwrap();
    let result = classified.paths(
        0.4,
        0.4,
        region::HatchAxis::X,
        &recipe,
        &mut (MAX_DEPOSITION_SEGMENTS as usize),
        &AtomicBool::new(false),
    );
    assert_eq!(
        result.err().unwrap().code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn native_section_consumer_preserves_hole_ownership_in_sparse_and_skin_paths() {
    let bytes = include_bytes!("../../../../fixtures/geometry/through-hole.step");
    let cancel = AtomicBool::new(false);
    let definition = spiling_geometry::import_step(bytes, &cancel).unwrap();
    let native = spiling_geometry::section_with_byte_limit(
        &definition,
        PlaneMm {
            origin_mm: [0.0, 0.0, 0.2],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
        4 * 1024 * 1024,
    )
    .unwrap();
    let section = region::from_native(native, &RigidPoseMm::IDENTITY, 0.2, &cancel).unwrap();
    assert_eq!(section.0.len(), 1);
    assert_eq!(section.0[0].interiors().len(), 1);
    let void = empty();
    for (below, above) in [(&section, &section), (&section, &void), (&void, &section)] {
        let (paths, coverage) = output(&section, below, above, 0.25, 0);
        partition(&coverage);
        assert!((coverage.protected_hole_area_mm2 - 9.0 * std::f64::consts::PI).abs() < 0.1);
        assert_eq!(
            paths
                .iter()
                .filter(|p| p.kind == DepositionKind::Perimeter)
                .count(),
            2
        );
        for edge in paths.iter().flat_map(|path| path.points_mm.windows(2)) {
            let a = [edge[0][0] - 10.0, edge[0][1] - 10.0];
            let d = [edge[1][0] - edge[0][0], edge[1][1] - edge[0][1]];
            let t = (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0.0, 1.0);
            assert!((a[0] + t * d[0]).hypot(a[1] + t * d[1]) >= 3.0 - 0.01);
        }
    }
}

#[test]
fn finite_density_endpoints_keep_solid_meaning_and_bounded_sparse_pitch() {
    let section = rectangle(20.0, 10.0);
    let (solid, full) = output(&section, &section, &section, 1.0, 0);
    assert!(solid.iter().any(|p| p.kind == DepositionKind::SolidFill));
    assert!(!solid.iter().any(|p| p.kind == DepositionKind::SparseFill));
    assert_eq!(full.intentional_void_area_mm2, 0.0);
    assert_eq!(full.sparse_interior_area_mm2, 0.0);
    for fraction in [f64::MIN_POSITIVE, f64::from_bits(1)] {
        let (sparse, coverage) = output(&section, &section, &section, fraction, 0);
        assert_eq!(
            sparse
                .iter()
                .filter(|p| p.kind == DepositionKind::SparseFill)
                .count(),
            1
        );
        assert!(coverage.nominal_deposition_volume_mm3.is_finite());
        assert!(
            sparse
                .iter()
                .flat_map(|p| &p.points_mm)
                .flatten()
                .all(|p| p.is_finite())
        );
        partition(&coverage);
    }
}

#[test]
fn shared_wall_layout_preserves_paths_and_ownership_across_skin_phases() {
    let section = rectangle(20.0, 10.0);
    let upper = rectangle(10.0, 10.0);
    let void = empty();
    let recipe = recipe(0.25);
    let cancel = AtomicBool::new(false);
    let layout = Arc::new(
        WallLayout::new(&section, &recipe, region::point_count(&section), &cancel).unwrap(),
    );
    for (below, above) in [
        (&void, &section),
        (&section, &section),
        (&section, &upper),
        (&upper, &section),
        (&section, &void),
    ] {
        for index in 0..2 {
            let cached =
                ClassifiedRegions::new(Arc::clone(&layout), below, above, &recipe, &cancel)
                    .unwrap()
                    .paths(
                        0.4 * (index + 1) as f64,
                        0.4,
                        region::HatchAxis::preferred(index),
                        &recipe,
                        &mut 0,
                        &cancel,
                    )
                    .unwrap();
            let uncached = output(&section, below, above, 0.25, index);
            assert_eq!(cached, uncached);
            partition(&cached.1);
        }
    }
}

#[test]
fn true_height_schedule_and_physical_skin_neighborhoods_are_bounded() {
    let schedule = layer_schedule(0.0, 1.05, 0.4, &[0.55]).unwrap();
    assert_eq!(schedule.len(), 4);
    for (layer, (z, h)) in
        schedule
            .iter()
            .zip([(0.4, 0.4), (0.55, 0.15), (0.8, 0.25), (1.05, 0.25)])
    {
        assert!((layer.z - z).abs() < 1e-12);
        assert!((layer.height - h).abs() < 1e-12);
        assert!((layer.sample - (z - h * 0.5)).abs() < 1e-12);
    }
    assert_eq!(physical_neighbor(&schedule, 3, 0.3, false), Some(1));
    assert_eq!(physical_neighbor(&schedule, 0, 0.3, true), Some(2));
    assert_eq!(
        layer_schedule(0.0, 1.0, 0.4, &[0.4001]).err().unwrap().code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
    assert_eq!(
        layer_schedule(0.0, 1e9, 0.4, &[]).err().unwrap().code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn native_event_roundoff_identity_includes_bed_grid_and_exact_top() {
    let top = 1.05;
    let events = [
        f64::EPSILON * 0.125,
        0.4 - f64::EPSILON,
        0.8 + f64::EPSILON,
        top - f64::EPSILON,
    ];
    let schedule = layer_schedule(0.0, top, 0.4, &events).unwrap();
    assert_eq!(
        schedule.iter().map(|layer| layer.z).collect::<Vec<_>>(),
        [0.4, 0.8, top]
    );
    assert_eq!(schedule[0].height, 0.4);
    for events in [&[0.0001][..], &[0.4001][..], &[top - 0.0001][..]] {
        assert_eq!(
            layer_schedule(0.0, top, 0.4, events).err().unwrap().code,
            ManufacturingErrorCode::UnsupportedGeometry
        );
    }
}

#[test]
fn deposited_footprint_holes_and_sparse_void_are_real_anchor_gaps() {
    let cancel = AtomicBool::new(false);
    let mut query = support::AnchorWorkspace::default();
    let anchors = [
        DepositionPath {
            kind: DepositionKind::SparseFill,
            points_mm: vec![[2.0, 0.0, 0.4], [2.0, 10.0, 0.4]],
        },
        DepositionPath {
            kind: DepositionKind::SparseFill,
            points_mm: vec![[8.0, 0.0, 0.4], [8.0, 10.0, 0.4]],
        },
    ];
    let gaps = query
        .gaps(
            support::AnchorCandidate {
                endpoints_mm: [[0.0, 5.0, 0.8], [10.0, 5.0, 0.8]],
                bead_width_mm: 0.6,
            },
            anchors.iter(),
            std::iter::empty(),
            &mut 0,
            &cancel,
        )
        .unwrap();
    assert_eq!(gaps.len(), 3);
    assert!(((gaps[1].end - gaps[1].start) * 10.0 - 5.4).abs() < 1e-10);
    let holed = [DepositionPath {
        kind: DepositionKind::Perimeter,
        points_mm: vec![
            [4.0, 4.0, 0.4],
            [6.0, 4.0, 0.4],
            [6.0, 6.0, 0.4],
            [4.0, 6.0, 0.4],
            [4.0, 4.0, 0.4],
        ],
    }];
    let gaps = query
        .gaps(
            support::AnchorCandidate {
                endpoints_mm: [[3.0, 5.0, 0.8], [7.0, 5.0, 0.8]],
                bead_width_mm: 0.6,
            },
            holed.iter(),
            std::iter::empty(),
            &mut 0,
            &cancel,
        )
        .unwrap();
    assert_eq!(gaps.len(), 3);
    assert!(((gaps[1].end - gaps[1].start) * 4.0 - 1.4).abs() < 1e-10);
}

#[test]
fn deposited_solid_non_grid_transition_has_actual_anchors_across_shrinking_wall() {
    let cancel = AtomicBool::new(false);
    let mut query = support::AnchorWorkspace::default();
    let recipe = recipe(1.0);
    let section = rectangle(20.0, 10.0);
    let paths = classify(&section, &section, &section, &recipe, &cancel)
        .unwrap()
        .paths(3.15, 0.35, region::HatchAxis::Y, &recipe, &mut 0, &cancel)
        .unwrap()
        .0;
    let gaps = query
        .gaps(
            support::AnchorCandidate {
                endpoints_mm: [[11.7, 0.3, 3.2], [11.7, 9.7, 3.2]],
                bead_width_mm: recipe.bead_width_mm,
            },
            paths.iter(),
            std::iter::empty(),
            &mut 0,
            &cancel,
        )
        .unwrap();
    assert!(
        gaps.is_empty(),
        "actual dense Y strokes plus walls leave unexpected shrinking-wall gaps {gaps:?}"
    );
}

#[test]
fn exact_deposited_capsules_keep_curved_caps_and_bounded_work() {
    let cancel = AtomicBool::new(false);
    let mut query = support::AnchorWorkspace::default();
    let paths = [DepositionPath {
        kind: DepositionKind::SparseFill,
        points_mm: vec![[0.0, 0.0, 0.4], [0.0, 10.0, 0.4]],
    }];
    let gaps = query
        .gaps(
            support::AnchorCandidate {
                endpoints_mm: [[-0.5, -0.1, 0.8], [0.5, -0.1, 0.8]],
                bead_width_mm: 0.6,
            },
            paths.iter(),
            std::iter::empty(),
            &mut 0,
            &cancel,
        )
        .unwrap();
    let extent = (0.3f64.powi(2) - 0.1f64.powi(2)).sqrt();
    assert_eq!(gaps.len(), 2);
    assert!((gaps[0].end - (0.5 - extent)).abs() < 1e-12);
    assert!((gaps[1].start - (0.5 + extent)).abs() < 1e-12);
    assert_eq!(
        query
            .gaps(
                support::AnchorCandidate {
                    endpoints_mm: [[-0.5, -0.1, 0.8], [0.5, -0.1, 0.8]],
                    bead_width_mm: 0.6,
                },
                paths.iter(),
                std::iter::empty(),
                &mut 20_000_000,
                &cancel
            )
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    assert_eq!(
        query
            .gaps(
                support::AnchorCandidate {
                    endpoints_mm: [[-0.5, -0.1, 0.8], [0.5, -0.1, 0.8]],
                    bead_width_mm: 0.6,
                },
                paths.iter(),
                std::iter::empty(),
                &mut 0,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
        ManufacturingErrorCode::Cancelled
    );
    query.set_budget(2);
    assert_eq!(
        query
            .gaps(
                support::AnchorCandidate {
                    endpoints_mm: [[-0.5, -0.1, 0.8], [0.5, -0.1, 0.8]],
                    bead_width_mm: 0.6,
                },
                paths.iter(),
                std::iter::empty(),
                &mut 0,
                &cancel
            )
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn support_raster_wall_center_entry_has_positive_previous_bead_overlap() {
    let cancel = AtomicBool::new(false);
    let mut query = support::AnchorWorkspace::default();
    let paths = [20.5, 21.5, 22.5].map(|x| DepositionPath {
        kind: DepositionKind::Support,
        points_mm: vec![[x, 0.3, 0.8], [x, 9.7, 0.8]],
    });
    let tangent = query
        .gaps(
            support::AnchorCandidate {
                endpoints_mm: [[20.8, 0.715339455, 1.2], [22.2, 0.715339455, 1.2]],
                bead_width_mm: 0.6,
            },
            paths.iter(),
            std::iter::empty(),
            &mut 0,
            &cancel,
        )
        .unwrap();
    assert!(tangent[0].start < 1e-8);
    assert!(((tangent[0].end - tangent[0].start) * 1.4 - 0.4).abs() < 1e-10);
    assert!(tangent[0].left && tangent[0].right && tangent[0].direct_ends);
    let entered = query
        .gaps(
            support::AnchorCandidate {
                endpoints_mm: [[20.5, 0.715339455, 1.2], [22.5, 0.715339455, 1.2]],
                bead_width_mm: 0.6,
            },
            paths.iter(),
            std::iter::empty(),
            &mut 0,
            &cancel,
        )
        .unwrap();
    assert!(
        entered
            .iter()
            .all(|gap| gap.start > 1e-8 && gap.end < 1.0 - 1e-8 && gap.direct_ends)
    );
    assert!(entered.iter().all(|gap| (gap.end - gap.start) * 2.0 <= 6.0));
}

#[test]
fn lower_interface_capsules_do_not_forge_direct_previous_layer_anchors() {
    let previous = [DepositionPath {
        kind: DepositionKind::BottomSkin,
        points_mm: vec![[0.0, 0.0, 0.8], [2.0, 0.0, 0.8]],
    }];
    let contact = [DepositionPath {
        kind: DepositionKind::SupportInterface,
        points_mm: vec![[8.0, 0.0, 0.6], [10.0, 0.0, 0.6]],
    }];
    let mut query = super::support::AnchorWorkspace::default();
    let cancel = AtomicBool::new(false);
    let gap = query
        .gaps(
            support::AnchorCandidate {
                endpoints_mm: [[0.0, 0.0, 1.2], [10.0, 0.0, 1.2]],
                bead_width_mm: 0.6,
            },
            previous.iter(),
            contact.iter(),
            &mut 0,
            &cancel,
        )
        .unwrap();
    assert_eq!(gap.len(), 1);
    assert!((gap[0].start - 0.23).abs() < 1e-12);
    assert!((gap[0].end - 0.77).abs() < 1e-12);
    assert!(!gap[0].direct_ends);
    let gap = query
        .gaps(
            support::AnchorCandidate {
                endpoints_mm: [[0.0, 0.0, 1.2], [10.0, 0.0, 1.2]],
                bead_width_mm: 0.6,
            },
            previous.iter().chain(contact.iter()),
            std::iter::empty(),
            &mut 0,
            &cancel,
        )
        .unwrap();
    assert_eq!(gap.len(), 1);
    assert!((gap[0].start - 0.23).abs() < 1e-12);
    assert!((gap[0].end - 0.77).abs() < 1e-12);
    assert!(gap[0].direct_ends);
}
