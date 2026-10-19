// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use classification::{ClassifiedRegions, WallLayout};
use geo::{Coord, LineString, Polygon};
use spiling_contracts::geometry::PlaneMm;
use std::collections::BTreeSet;

pub(super) fn recipe(fraction: f64) -> PlanarPrintRecipe {
    PlanarPrintRecipe {
        native_shape_error_mm: 0.1,
        id: "region-test".into(),
        revision: "1".into(),
        material_id: "nominal".into(),
        material_revision: "1".into(),
        filament_diameter_mm: 1.75,
        layer_height_mm: 0.4,
        bead_width_mm: 0.6,
        perimeter_count: 1,
        seam_policy: SeamPolicy::Nearest,
        rectilinear_axis: RectilinearAxisPolicy::AlternatingXY,
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
        &region::RequiredMaterial::from_section(section),
        recipe,
        region::point_count(section),
        cancel,
    )?);
    let inset = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
    let below = region::offset(below, -inset, cancel)?;
    let above = region::offset(above, -inset, cancel)?;
    let retained_points = region::point_count(section)
        + layout.point_count()
        + region::point_count(&below)
        + region::point_count(&above);
    ClassifiedRegions::new(
        layout,
        &below,
        &above,
        classification::fixed_point_budget(retained_points),
        recipe,
        cancel,
    )
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
    let regions = classify(section, below, above, &recipe, &cancel).unwrap();
    own_output(
        regions
            .paths(
                0.4 * (index + 1) as f64,
                0.4,
                region::HatchAxis::for_policy(recipe.rectilinear_axis, index),
                &recipe,
                &mut 0,
                &cancel,
            )
            .unwrap(),
        0.4,
        &recipe,
    )
}
fn own_output(
    (paths, coverage): (Vec<DepositionPath>, &PlanarRegionCoverage),
    height: f64,
    recipe: &PlanarPrintRecipe,
) -> (Vec<DepositionPath>, PlanarRegionCoverage) {
    let mut coverage = coverage.clone();
    let area = ((recipe.bead_width_mm - height) * height
        + std::f64::consts::PI * (height * 0.5).powi(2))
        * recipe.flow_multiplier;
    coverage.nominal_deposition_volume_mm3 = paths
        .iter()
        .flat_map(|p| p.points_mm.windows(2))
        .map(|e| (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1]) * area)
        .sum();
    (paths, coverage)
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
fn thick_sampled_boundary_neighborhood_preserves_expected_exterior_hole_and_clearance() {
    use geo::{Contains, Point};
    let cancel = AtomicBool::new(false);
    for center in [[0.0, 0.0], [110.0, 110.0]] {
        let circle = |radius: f64, samples: usize| {
            let mut points = (0..samples)
                .map(|i| {
                    let theta = std::f64::consts::TAU * i as f64 / samples as f64;
                    Coord {
                        x: center[0] + radius * theta.cos(),
                        y: center[1] + radius * theta.sin(),
                    }
                })
                .collect::<Vec<_>>();
            points.push(points[0]);
            LineString(points)
        };
        let mut material =
            MultiPolygon(vec![Polygon::new(circle(5.0, 314), vec![circle(1.0, 96)])]);
        region::canonicalize(&mut material);
        let original = material.clone();
        let eroded = region::offset(&material, -0.9, &cancel).unwrap();
        assert_eq!(material, original);
        assert_eq!(eroded.0.len(), 1);
        assert_eq!(eroded.0[0].interiors().len(), 1);
        assert!(!eroded.contains(&Point(Coord {
            x: center[0],
            y: center[1]
        })));
        for (ring, expected) in std::iter::once((eroded.0[0].exterior(), 4.1))
            .chain(eroded.0[0].interiors().iter().map(|ring| (ring, 1.9)))
        {
            for point in &ring.0 {
                let radius = (point.x - center[0]).hypot(point.y - center[1]);
                // Source polygon sagitta stays below.001 independently of the
                // unchanged.005 native offset policy.
                assert!((radius - expected).abs() <= OFFSET_TOLERANCE_MM + 0.001);
                assert!(material.contains(&Point(*point)));
            }
        }
        let expected_area = std::f64::consts::PI * (4.1f64.powi(2) - 1.9f64.powi(2));
        let radial_area_bound = std::f64::consts::TAU * (4.1 + 1.9) * (OFFSET_TOLERANCE_MM + 0.001);
        assert!((eroded.unsigned_area() - expected_area).abs() <= radial_area_bound);
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
            material: Arc::new(rectangle(20.0, 10.0)),
            process_domains: Vec::new(),
            occupied: None,
            source_area_mm2: 200.0,
        },
        SectionSpan {
            start: 6,
            end: 12,
            material: Arc::new(rectangle(10.0, 10.0)),
            process_domains: Vec::new(),
            occupied: None,
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
    let retained_points = sections
        .iter()
        .map(|span| region::point_count(&span.material))
        .sum();
    let inset = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
    let mut neighborhoods = SkinNeighborhoods::new(&sections, inset, 0).unwrap();
    for index in 0..12u32 {
        let current = span_at(&sections, index);
        let below = physical_neighbor(&schedule, index, recipe.bottom_skin_thickness_mm, false)
            .map(|i| span_at(&sections, i));
        let above = physical_neighbor(&schedule, index, recipe.top_skin_thickness_mm, true)
            .map(|i| span_at(&sections, i));
        let lower = neighborhoods
            .survival(current, below, retained_points, &cancel)
            .unwrap();
        let upper = neighborhoods
            .survival(
                current,
                above,
                skin_query_retained_points(retained_points, &[&lower]).unwrap(),
                &cancel,
            )
            .unwrap();
        let section = &sections[current].material;
        let layout = Arc::new(
            WallLayout::new(
                section,
                &region::RequiredMaterial::from_section(section),
                &recipe,
                skin_retained_points(retained_points, &neighborhoods, &[&lower, &upper]).unwrap(),
                &cancel,
            )
            .unwrap(),
        );
        let layout_points = layout.point_count();
        let classified = ClassifiedRegions::new(
            layout,
            &lower,
            &upper,
            |scratch| {
                admit_skin_scratch(
                    retained_points + layout_points,
                    &mut neighborhoods,
                    &[&lower, &upper],
                    scratch,
                    &cancel,
                )
            },
            &recipe,
            &cancel,
        )
        .unwrap();
        let (_, coverage) = classified
            .paths(
                schedule[index as usize].z,
                schedule[index as usize].height,
                region::HatchAxis::for_policy(recipe.rectilinear_axis, index),
                &recipe,
                &mut 0,
                &cancel,
            )
            .unwrap();
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
    let section = region::from_native(&native, &RigidPoseMm::IDENTITY, 0.2, &cancel).unwrap();
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
        WallLayout::new(
            &section,
            &region::RequiredMaterial::from_section(&section),
            &recipe,
            region::point_count(&section),
            &cancel,
        )
        .unwrap(),
    );
    for (below, above) in [
        (&void, &section),
        (&section, &section),
        (&section, &upper),
        (&upper, &section),
        (&section, &void),
    ] {
        let inset = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
        let below_inset = region::offset(below, -inset, &cancel).unwrap();
        let above_inset = region::offset(above, -inset, &cancel).unwrap();
        for index in 0..2 {
            let regions = ClassifiedRegions::new(
                Arc::clone(&layout),
                &below_inset,
                &above_inset,
                classification::fixed_point_budget(
                    region::point_count(&section)
                        + layout.point_count()
                        + region::point_count(&below_inset)
                        + region::point_count(&above_inset),
                ),
                &recipe,
                &cancel,
            )
            .unwrap();
            let cached = own_output(
                regions
                    .paths(
                        0.4 * (index + 1) as f64,
                        0.4,
                        region::HatchAxis::for_policy(recipe.rectilinear_axis, index),
                        &recipe,
                        &mut 0,
                        &cancel,
                    )
                    .unwrap(),
                0.4,
                &recipe,
            );
            let uncached = output(&section, below, above, 0.25, index);
            assert_eq!(cached, uncached);
            partition(&cached.1);
        }
    }
}

#[test]
fn true_height_schedule_and_physical_skin_neighborhoods_are_bounded() {
    let schedule = layer_schedule(0.0, 1.05, 0.4, &[0.55]).unwrap();
    let mut previous = 0.0;
    for layer in &schedule {
        assert!(layer.height > 0.0 && layer.height <= 0.4);
        assert_eq!(layer.sample, previous + (layer.z - previous) * 0.5);
        assert!(layer.sample > previous && layer.sample < layer.z);
        previous = layer.z;
    }
    assert!(schedule.iter().any(|layer| layer.z == 0.55));
    assert_eq!(schedule.last().unwrap().z, 1.05);
    for (start, end) in [(0.0, 0.55), (0.55, 1.05)] {
        let mut minimum = f64::INFINITY;
        let mut maximum = 0.0_f64;
        for layer in schedule
            .iter()
            .filter(|layer| layer.z > start && layer.z <= end)
        {
            minimum = minimum.min(layer.height);
            maximum = maximum.max(layer.height);
        }
        assert!(maximum - minimum <= 4.0 * f64::EPSILON);
    }
    assert_eq!(physical_neighbor(&schedule, 3, 0.3, false), Some(1));
    assert_eq!(physical_neighbor(&schedule, 0, 0.3, true), Some(2));
    assert_eq!(
        layer_schedule(0.0, 1.0, 0.4, &[0.4, 0.4001])
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::UnsupportedGeometry
    );
    assert_eq!(
        layer_schedule(0.0, 1e9, 0.4, &[]).err().unwrap().code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn native_event_roundoff_identity_preserves_genuine_events_and_exact_top() {
    let top = 1.05;
    let first = 0.4 - f64::EPSILON;
    let second = 0.8 + f64::EPSILON;
    let events = [
        f64::EPSILON * 0.125,
        first,
        0.4 + f64::EPSILON,
        second,
        top - f64::EPSILON,
    ];
    let schedule = layer_schedule(0.0, top, 0.4, &events).unwrap();
    assert_eq!(schedule.len(), 3, "arithmetic identity must not add slabs");
    assert!(schedule.iter().any(|layer| layer.z == first));
    assert!(schedule.iter().any(|layer| layer.z == second));
    assert_eq!(schedule.last().unwrap().z, top);
    for events in [&[0.0001][..], &[0.4, 0.4001][..], &[top - 0.0001][..]] {
        assert_eq!(
            layer_schedule(0.0, top, 0.4, events).err().unwrap().code,
            ManufacturingErrorCode::UnsupportedGeometry
        );
    }
}

#[test]
fn phase_offset_dense_row_events_do_not_create_nominal_grid_slivers() {
    // A 3.7-mm, 32-row image has this row spacing. Offset its genuine
    // boundaries so some fall only 0.000125 mm above the old nominal grid.
    let top = 8.0;
    let events: Vec<_> = (0..70)
        .map(|row| 0.100125 + row as f64 * (3.7 / 32.0))
        .filter(|event| *event < top)
        .collect();
    assert!((events[32] - 3.8 - 0.000125).abs() < 1e-12);
    let schedule = layer_schedule(0.0, top, 0.2, &events).unwrap();
    let mut previous = 0.0;
    for layer in &schedule {
        assert!(layer.height > 0.03, "no artificial nominal-grid sliver");
        assert!(layer.height <= 0.2);
        assert_eq!(layer.sample, previous + (layer.z - previous) * 0.5);
        assert!(layer.sample > previous && layer.sample < layer.z);
        previous = layer.z;
    }
    for event in events {
        assert!(schedule.iter().any(|layer| layer.z == event));
    }
    assert_eq!(schedule.last().unwrap().z, top);
}

#[test]
fn integral_decimal_ratios_use_roundoff_safe_layer_budget_admission() {
    for bed in [0.0, 17.0, -17.0] {
        for count in [3, 170, MAX_PRINT_LAYERS as usize] {
            let top = bed + count as f64 * 0.2;
            let schedule = layer_schedule(bed, top, 0.2, &[]).unwrap();
            assert_eq!(schedule.len(), count, "roundoff must not add a layer");
            assert_eq!(schedule.last().unwrap().z, top);
            let construction_roundoff =
                4.0 * f64::EPSILON * bed.abs().max(top.abs()).max((top - bed).abs()).max(1.0);
            let mut previous = bed;
            for layer in schedule {
                assert_eq!(
                    layer.lower_z, previous,
                    "native slab closures must meet without a normalized-height gap"
                );
                assert!(layer.height > 0.0 && layer.height <= 0.2);
                assert!((layer.z - previous - layer.height).abs() <= construction_roundoff);
                assert_eq!(layer.sample, previous + (layer.z - previous) * 0.5);
                previous = layer.z;
            }
        }
    }
    let events: Vec<_> = (1..=MAX_PRINT_LAYERS).map(|index| index as f64).collect();
    assert_eq!(
        layer_schedule(0.0, MAX_PRINT_LAYERS as f64 + 1.0, 1.0, &events)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn deposited_footprint_holes_and_sparse_void_are_real_anchor_gaps() {
    let cancel = AtomicBool::new(false);
    let mut query = support::AnchorWorkspace::default();
    let anchors = [
        DepositionPath {
            kind: DepositionKind::SparseFill,
            owner: None,
            points_mm: vec![[2.0, 0.0, 0.4], [2.0, 10.0, 0.4]],
        },
        DepositionPath {
            kind: DepositionKind::SparseFill,
            owner: None,
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
        owner: None,
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
        owner: None,
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
        owner: None,
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
        owner: Some(
            spiling_contracts::manufacturing::DepositionOwner::MandatorySkin {
                skin: spiling_contracts::manufacturing::ProcessSkinKind::BottomSkin,
            },
        ),
        points_mm: vec![[0.0, 0.0, 0.8], [2.0, 0.0, 0.8]],
    }];
    let contact = [DepositionPath {
        kind: DepositionKind::SupportInterface,
        owner: None,
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

#[test]
fn decimal_nominal_bead_height_keeps_strict_limit_and_exact_event_endpoints() {
    for bed in [0.0, 17.0, -17.0] {
        let events = [bed + 4.0, bed + 6.0, bed + 7.0, bed + 7.8];
        let schedule = layer_schedule(bed, bed + 8.0, 0.6, &events).unwrap();
        let mut previous = bed;
        for layer in &schedule {
            assert_eq!(layer.lower_z, previous);
            assert!(
                layer.height <= 0.6,
                "subdivision roundoff must not exceed the strict bead cap"
            );
            let allowance = 4.0 * f64::EPSILON * layer.z.abs().max(previous.abs()).max(1.0);
            assert!((layer.z - previous - layer.height).abs() <= allowance);
            assert_eq!(layer.sample, previous + (layer.z - previous) * 0.5);
            assert!(layer.sample > previous && layer.sample < layer.z);
            previous = layer.z;
        }
        assert_eq!(schedule.last().unwrap().z, bed + 8.0);
        for event in events {
            assert!(schedule.iter().any(|layer| layer.z == event));
        }
    }
}

#[test]
fn required_coverage_keeps_native_thin_regions_hard_and_requested_fine_lobes_advisory() {
    let cancel = AtomicBool::new(false);
    let recipe = recipe(1.0);
    let shape = |points: &[[f64; 2]]| {
        MultiPolygon(vec![Polygon::new(
            LineString(
                points
                    .iter()
                    .map(|point| Coord {
                        x: point[0],
                        y: point[1],
                    })
                    .collect(),
            ),
            Vec::new(),
        )])
    };
    let native = rectangle(20.0, 10.0);
    let requested = shape(&[
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 10.0],
        [10.1, 10.0],
        [10.0, 10.3],
        [9.9, 10.0],
        [0.0, 10.0],
        [0.0, 0.0],
    ]);
    let retained = region::point_count(&native) + region::point_count(&requested);
    WallLayout::new(
        &requested,
        &region::RequiredMaterial::from_section(&native),
        &recipe,
        retained,
        &cancel,
    )
    .unwrap();
    let strict = WallLayout::new(
        &requested,
        &region::RequiredMaterial::from_section(&requested),
        &recipe,
        retained,
        &cancel,
    )
    .err()
    .unwrap();
    assert_eq!(
        strict.code,
        ManufacturingErrorCode::UnsupportedGeometry,
        "the same produced erosion must still refuse unreachable REQUIRED material"
    );
    let native_thin = shape(&[
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 10.0],
        [6.05, 10.0],
        [6.05, 12.0],
        [5.95, 12.0],
        [5.95, 10.0],
        [0.0, 10.0],
        [0.0, 0.0],
    ]);
    let requested_thin = shape(&[
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 10.0],
        [10.1, 10.0],
        [10.0, 10.3],
        [9.9, 10.0],
        [6.05, 10.0],
        [6.05, 12.0],
        [5.95, 12.0],
        [5.95, 10.0],
        [0.0, 10.0],
        [0.0, 0.0],
    ]);
    let thin = WallLayout::new(
        &requested_thin,
        &region::RequiredMaterial::from_section(&native_thin),
        &recipe,
        region::point_count(&native_thin) + region::point_count(&requested_thin),
        &cancel,
    )
    .err()
    .unwrap();
    assert_eq!(
        thin.code,
        ManufacturingErrorCode::UnsupportedGeometry,
        "a requested fine addition must not disable native-M thin-region admission"
    );
}

#[test]
fn mixed_face_required_coverage_keeps_ribs_hard_on_the_image_owned_island() {
    let cancel = AtomicBool::new(false);
    let recipe = recipe(1.0);
    let polygon = |points: &[[f64; 2]]| {
        Polygon::new(
            LineString(
                points
                    .iter()
                    .map(|point| Coord {
                        x: point[0],
                        y: point[1],
                    })
                    .collect(),
            ),
            Vec::new(),
        )
    };
    let native = rectangle(20.0, 10.0);
    let unrelated = polygon(&[
        [40.0, 0.0],
        [60.0, 0.0],
        [60.0, 10.0],
        [40.0, 10.0],
        [40.0, 0.0],
    ]);
    let fine_image = polygon(&[
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 10.0],
        [10.1, 10.0],
        [10.0, 10.3],
        [9.9, 10.0],
        [0.0, 10.0],
        [0.0, 0.0],
    ]);
    // The narrow rib is on the right face of the SAME island whose top face
    // carries advisory fine image detail. It is not an image displacement.
    let rib = polygon(&[
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 4.95],
        [22.0, 4.95],
        [22.0, 5.05],
        [20.0, 5.05],
        [20.0, 10.0],
        [0.0, 10.0],
        [0.0, 0.0],
    ]);
    let mixed = polygon(&[
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 4.95],
        [22.0, 4.95],
        [22.0, 5.05],
        [20.0, 5.05],
        [20.0, 10.0],
        [10.1, 10.0],
        [10.0, 10.3],
        [9.9, 10.0],
        [0.0, 10.0],
        [0.0, 0.0],
    ]);
    let id = spiling_contracts::geometry::OccurrenceId::new(1).unwrap();
    let mut original = MultiPolygon(vec![unrelated.clone(), native.0[0].clone()]);
    let native_bytes = native::material_bytes(&mut original);
    let original = Arc::new(original);
    let mut span = NativeSurfaceSpan {
        start: 0,
        end: 1,
        occurrences: vec![(id, Arc::clone(&original))],
        image_islands: vec![(id, 1)],
        required: BTreeMap::from([(id, Arc::clone(&original))]),
        native_bytes,
    };
    let mut work = 0;
    let image_requested = MultiPolygon(vec![fine_image, unrelated.clone()]);
    let required = required_material(&image_requested, Some(&span), &mut work, &cancel).unwrap();
    WallLayout::new(&image_requested, &required, &recipe, 100, &cancel).unwrap();

    // C and D deliberately use a different order from original M. Attribution
    // must follow M's exact witness, not matching polygon indices.
    span.required
        .insert(id, Arc::new(MultiPolygon(vec![rib, unrelated.clone()])));
    let requested = MultiPolygon(vec![mixed, unrelated]);
    let required = required_material(&requested, Some(&span), &mut work, &cancel).unwrap();
    let failure = WallLayout::new(&requested, &required, &recipe, 100, &cancel)
        .err()
        .expect("unreachable non-image rib material must refuse compilation");
    assert_eq!(failure.code, ManufacturingErrorCode::UnsupportedGeometry);
    assert!(
        failure.message.contains("thin regions unreachable"),
        "mixed rib coverage must fail the actual hard reachability gate: {}",
        failure.message
    );
}

#[test]
fn skin_horizon_intersects_varying_local_boundaries_under_cumulative_work_cap() {
    use geo::{Contains, Point};
    let cancel = AtomicBool::new(false);
    let sections: Vec<_> = (0..96)
        .map(|index| {
            let radius = 10.0 - index as f64 * 0.01;
            let mut exterior: Vec<_> = (0..512)
                .map(|vertex| {
                    let angle = std::f64::consts::TAU * vertex as f64 / 512.0;
                    Coord {
                        x: radius * angle.cos(),
                        y: radius * angle.sin(),
                    }
                })
                .collect();
            exterior.push(exterior[0]);
            let material = MultiPolygon(vec![Polygon::new(
                LineString(exterior),
                vec![ring(-1.0, -1.0, 2.0, 2.0)],
            )]);
            SectionSpan {
                occupied: None,
                start: index,
                end: index + 1,
                source_area_mm2: material.unsigned_area(),
                material: Arc::new(material),
                process_domains: Vec::new(),
            }
        })
        .collect();
    // A whole-ring Cartesian estimate exceeds20M for this horizon, although
    // nearby annular boundaries have bounded local overlay candidates.
    let retained_points = sections
        .iter()
        .map(|span| region::point_count(&span.material))
        .sum();
    let mut neighborhoods = SkinNeighborhoods::new(&sections, 0.0, 0).unwrap();
    let actual = neighborhoods
        .survival(0, Some(sections.len() - 1), retained_points, &cancel)
        .unwrap();
    let radius = 10.0 - 95.0 * 0.01;
    let expected = 256.0 * radius * radius * (std::f64::consts::TAU / 512.0).sin() - 4.0;
    assert!(
        (actual.unsigned_area() - expected).abs() < 0.0001,
        "the complete horizon must retain exactly its smallest material domain"
    );
    assert!(actual.contains(&Point::new(2.0, 0.0)));
    assert!(
        !actual.contains(&Point::new(0.0, 0.0)),
        "protected hole must survive every overlay"
    );
}

#[test]
fn sliding_physical_skin_horizons_reuse_varying_annuli_and_protected_holes() {
    use geo::{BooleanOps, Contains, Point};

    const VERTICES: usize = 512;
    const LAYERS: usize = 64;
    fn radius(index: usize) -> f64 {
        9.0 + ((index * 13) % 29) as f64 * 0.025
    }
    fn annulus(radius: f64) -> MultiPolygon<f64> {
        let mut exterior: Vec<_> = (0..VERTICES)
            .map(|vertex| {
                let angle = std::f64::consts::TAU * vertex as f64 / VERTICES as f64;
                Coord {
                    x: radius * angle.cos(),
                    y: radius * angle.sin(),
                }
            })
            .collect();
        exterior.push(exterior[0]);
        MultiPolygon(vec![Polygon::new(
            LineString(exterior),
            vec![ring(-1.0, -1.0, 2.0, 2.0)],
        )])
    }
    let sections: Vec<_> = (0..LAYERS)
        .map(|index| {
            let material = annulus(radius(index));
            SectionSpan {
                occupied: None,
                start: index as u32,
                end: index as u32 + 1,
                source_area_mm2: material.unsigned_area(),
                material: Arc::new(material),
                process_domains: Vec::new(),
            }
        })
        .collect();
    let cancel = AtomicBool::new(false);
    let mut recipe = recipe(0.3);
    recipe.bottom_skin_thickness_mm = 3.2;
    recipe.top_skin_thickness_mm = 3.2;
    let schedule = layer_schedule(0.0, 12.8, 0.2, &[]).unwrap();
    assert_eq!(schedule.len(), LAYERS);
    // Exact nominal physical endpoints include every intervening D, not a
    // native-slab identity or a fixed number of cached source footprints.
    assert_eq!(physical_neighbor(&schedule, 16, 3.2, false), Some(0));
    assert_eq!(physical_neighbor(&schedule, 16, 3.2, true), Some(32));
    let mut retained_points: usize = sections
        .iter()
        .map(|span| region::point_count(&span.material))
        .sum();
    let mut wall_layouts = BTreeMap::new();
    let initial_work = 100_000;
    let inset = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
    // Independent oracle: derive every final-D leaf, then brute-force each
    // physical range. No native/source identity or minimum-radius recognition.
    let derived: Vec<_> = sections
        .iter()
        .map(|span| region::offset(&span.material, -inset, &cancel).unwrap())
        .collect();
    let brute_force = |current: usize, neighbor: Option<usize>| {
        let Some(neighbor) = neighbor else {
            return empty();
        };
        let first = current.min(neighbor);
        let last = current.max(neighbor);
        derived[first + 1..last + 1]
            .iter()
            .fold(derived[first].clone(), |value, leaf| {
                classification::intersection(&value, leaf, &cancel).unwrap()
            })
    };
    let mut neighborhoods = SkinNeighborhoods::new(&sections, inset, initial_work).unwrap();
    for index in 0..LAYERS {
        let below = physical_neighbor(&schedule, index as u32, 3.2, false)
            .map(|layer| span_at(&sections, layer));
        let above = physical_neighbor(&schedule, index as u32, 3.2, true)
            .map(|layer| span_at(&sections, layer));
        let lower = neighborhoods
            .survival(index, below, retained_points, &cancel)
            .unwrap();
        let upper = neighborhoods
            .survival(
                index,
                above,
                skin_query_retained_points(retained_points, &[&lower]).unwrap(),
                &cancel,
            )
            .unwrap();
        let expected = [brute_force(index, below), brute_force(index, above)];
        for (actual, expected) in [(&*lower, &expected[0]), (&*upper, &expected[1])] {
            assert!((actual.unsigned_area() - expected.unsigned_area()).abs() < 0.0001);
            assert!(actual.xor(expected).unsigned_area() < 0.0001);
            assert!(!actual.contains(&Point::new(0.0, 0.0)));
        }
        if [16, 32, 47].contains(&index) {
            let layout = Arc::new(
                WallLayout::new(
                    &sections[index].material,
                    &region::RequiredMaterial::from_section(&sections[index].material),
                    &recipe,
                    skin_retained_points(retained_points, &neighborhoods, &[&lower, &upper])
                        .unwrap(),
                    &cancel,
                )
                .unwrap(),
            );
            retained_points += layout.point_count();
            wall_layouts.insert(index, Arc::clone(&layout));
            let actual = ClassifiedRegions::new(
                Arc::clone(&layout),
                &lower,
                &upper,
                |scratch| {
                    admit_skin_scratch(
                        retained_points,
                        &mut neighborhoods,
                        &[&lower, &upper],
                        scratch,
                        &cancel,
                    )
                },
                &recipe,
                &cancel,
            )
            .unwrap();
            let expected = ClassifiedRegions::new(
                layout,
                &expected[0],
                &expected[1],
                classification::fixed_point_budget(0),
                &recipe,
                &cancel,
            )
            .unwrap();
            for axis in [region::HatchAxis::X, region::HatchAxis::Y] {
                let (_, actual_coverage) = actual
                    .paths(
                        schedule[index].z,
                        schedule[index].height,
                        axis,
                        &recipe,
                        &mut 0,
                        &cancel,
                    )
                    .unwrap();
                let (_, expected_coverage) = expected
                    .paths(
                        schedule[index].z,
                        schedule[index].height,
                        axis,
                        &recipe,
                        &mut 0,
                        &cancel,
                    )
                    .unwrap();
                partition(&actual_coverage);
                for (actual_area, expected_area) in [
                    (
                        actual_coverage.bottom_skin_area_mm2,
                        expected_coverage.bottom_skin_area_mm2,
                    ),
                    (
                        actual_coverage.top_skin_area_mm2,
                        expected_coverage.top_skin_area_mm2,
                    ),
                    (
                        actual_coverage.sparse_interior_area_mm2,
                        expected_coverage.sparse_interior_area_mm2,
                    ),
                ] {
                    assert!((actual_area - expected_area).abs() < 0.0001);
                }
                assert!((actual_coverage.protected_hole_area_mm2 - 4.0).abs() < 0.0001);
            }
        }
    }
    assert!(*neighborhoods.work_mut() > initial_work);
    // Both support-regenerated axes reuse the same final-D derived cache inside
    // its frozen reservation, including any necessary charged recomputation.
    let pool = neighborhoods.freeze_retained_capacity();
    for index in [16, 32, 47] {
        let below =
            physical_neighbor(&schedule, index, 3.2, false).map(|layer| span_at(&sections, layer));
        let above =
            physical_neighbor(&schedule, index, 3.2, true).map(|layer| span_at(&sections, layer));
        for axis in [region::HatchAxis::X, region::HatchAxis::Y] {
            let lower = neighborhoods
                .survival(index as usize, below, retained_points, &cancel)
                .unwrap();
            let upper = neighborhoods
                .survival(
                    index as usize,
                    above,
                    skin_query_retained_points(retained_points, &[&lower]).unwrap(),
                    &cancel,
                )
                .unwrap();
            let classified = ClassifiedRegions::new(
                Arc::clone(&wall_layouts[&(index as usize)]),
                &lower,
                &upper,
                |scratch| {
                    admit_skin_scratch(
                        retained_points,
                        &mut neighborhoods,
                        &[&lower, &upper],
                        scratch,
                        &cancel,
                    )
                },
                &recipe,
                &cancel,
            )
            .unwrap();
            let (_, coverage) = classified
                .paths(
                    schedule[index as usize].z,
                    schedule[index as usize].height,
                    axis,
                    &recipe,
                    &mut 0,
                    &cancel,
                )
                .unwrap();
            partition(&coverage);
            assert!((coverage.protected_hole_area_mm2 - 4.0).abs() < 0.0001);
            assert!(neighborhoods.retained_points() <= pool);
        }
    }
    assert!(*neighborhoods.work_mut() <= MAX_BOOLEAN_PAIRS);
}

#[test]
fn classification_reclaims_optional_ranges_before_refusing_live_scratch() {
    let cancel = AtomicBool::new(false);
    let recipe = recipe(1.0);
    let section = rectangle(20.0, 10.0);
    let sections: Vec<_> = (0..2)
        .map(|index| SectionSpan {
            occupied: None,
            start: index,
            end: index + 1,
            material: Arc::new(section.clone()),
            process_domains: Vec::new(),
            source_area_mm2: 200.0,
        })
        .collect();
    let inset = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
    let mut neighborhoods = neighborhood::SkinNeighborhoods::new(&sections, inset, 0).unwrap();
    drop(neighborhoods.survival(0, Some(1), 0, &cancel).unwrap());
    let layout = Arc::new(
        WallLayout::new(
            &section,
            &region::RequiredMaterial::from_section(&section),
            &recipe,
            region::point_count(&section),
            &cancel,
        )
        .unwrap(),
    );
    let neighbor = region::offset(&section, -inset, &cancel).unwrap();
    let retained = MAX_DEPOSITION_SEGMENTS as usize - 5;
    let failure = ClassifiedRegions::new(
        Arc::clone(&layout),
        &neighbor,
        &neighbor,
        classification::fixed_point_budget(retained + neighborhoods.retained_points()),
        &recipe,
        &cancel,
    )
    .err()
    .unwrap();
    assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
    let classified = ClassifiedRegions::new(
        layout,
        &neighbor,
        &neighbor,
        |scratch| admit_skin_scratch(retained, &mut neighborhoods, &[], scratch, &cancel),
        &recipe,
        &cancel,
    )
    .unwrap();
    let (_, coverage) = classified
        .paths(0.4, 0.4, region::HatchAxis::X, &recipe, &mut 0, &cancel)
        .unwrap();
    assert!((coverage.solid_interior_area_mm2 - 18.8 * 8.8).abs() < 0.5);
    assert_eq!(coverage.section_area_mm2, 200.0);
    assert_eq!(coverage.protected_hole_area_mm2, 0.0);
}
