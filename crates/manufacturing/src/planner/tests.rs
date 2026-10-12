// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use classification::{ClassifiedRegions, WallLayout};
use geo::{Coord, LineString, Polygon};

fn recipe(fraction: f64) -> PlanarPrintRecipe {
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
        .paths(0.4 * (index + 1) as f64, index, &recipe, &mut 0, &cancel)
        .unwrap()
}
fn partition(coverage: &PlanarRegionCoverage) {
    let sum = coverage.wall_area_mm2
        + coverage.bottom_skin_area_mm2
        + coverage.top_skin_area_mm2
        + coverage.solid_interior_area_mm2
        + coverage.sparse_interior_area_mm2;
    assert!((sum - coverage.section_area_mm2).abs() < 1e-7);
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
        },
        SectionSpan {
            start: 6,
            end: 12,
            material: rectangle(10.0, 10.0),
        },
    ];
    let recipe = recipe(0.3);
    let top_layers = skin_layers(recipe.top_skin_thickness_mm, recipe.layer_height_mm, 12);
    let bottom_layers = skin_layers(recipe.bottom_skin_thickness_mm, recipe.layer_height_mm, 12);
    assert_eq!(top_layers, 3);
    assert_eq!(bottom_layers, 2);
    let cancel = AtomicBool::new(false);
    let mut work = 0;
    for index in 0..12u32 {
        let current = span_at(&sections, index);
        let below = index
            .checked_sub(bottom_layers)
            .map(|i| span_at(&sections, i));
        let above = (index + top_layers < 12).then(|| span_at(&sections, index + top_layers));
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
        0,
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
        (&section, &void),
    ] {
        for index in 0..2 {
            let cached =
                ClassifiedRegions::new(Arc::clone(&layout), below, above, &recipe, &cancel)
                    .unwrap()
                    .paths(0.4 * (index + 1) as f64, index, &recipe, &mut 0, &cancel)
                    .unwrap();
            let uncached = output(&section, below, above, 0.25, index);
            assert_eq!(cached, uncached);
            partition(&cached.1);
        }
    }
}
