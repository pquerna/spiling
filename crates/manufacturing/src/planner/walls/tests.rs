// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use geo::{Distance, Euclidean, Line, polygon};

fn plan(section: &MultiPolygon<f64>) -> WallPlan {
    let mut recipe = crate::planner::tests::recipe(0.25);
    recipe.bead_width_mm = 0.45;
    recipe.layer_height_mm = 0.2;
    recipe.perimeter_count = 2;
    let limits = DepositionLimits {
        min_bead_width_mm: 0.30,
        max_bead_width_mm: 0.75,
        min_bead_height_mm: 0.000001,
        max_bead_height_mm: 0.30,
    };
    let required = region::RequiredMaterial::from_section(section);
    plan_section_walls(
        WallRequest {
            material: section,
            required: &required,
            recipe: &recipe,
            limits: &limits,
            nozzle_diameter_mm: 0.4,
            owned_bytes: 0,
            lower_z: 0.0,
            upper_z: 0.2,
            retained_points: region::point_count(section),
        },
        &mut 0,
        &AtomicBool::new(false),
    )
    .unwrap()
}

fn cross_section(plan: &WallPlan, axis: usize, x: f64) -> (usize, Vec<[f64; 2]>) {
    let mut crossings = 0;
    let mut intervals = Vec::new();
    for path in &plan.paths {
        for edge in path.checked_edges().unwrap() {
            let a = [edge.points_mm[0][axis], edge.points_mm[0][1 - axis]];
            let b = [edge.points_mm[1][axis], edge.points_mm[1][1 - axis]];
            crossings += usize::from((a[0] <= x && x < b[0]) || (b[0] <= x && x < a[0]));
            let radius = edge.width_mm * 0.5;
            for endpoint in [a, b] {
                let dx = x - endpoint[0];
                if dx.abs() <= radius {
                    let dy = (radius * radius - dx * dx).max(0.0).sqrt();
                    intervals.push([endpoint[1] - dy, endpoint[1] + dy]);
                }
            }
            let direction = sub(b, a);
            let length = norm(direction);
            let n = [
                -direction[1] * radius / length,
                direction[0] * radius / length,
            ];
            let corners = [add(a, n), add(b, n), sub(b, n), sub(a, n)];
            let mut extent = [f64::INFINITY, f64::NEG_INFINITY];
            for index in 0..4 {
                let p = corners[index];
                let q = corners[(index + 1) % 4];
                if p[0] == q[0] {
                    if x == p[0] {
                        extent[0] = extent[0].min(p[1].min(q[1]));
                        extent[1] = extent[1].max(p[1].max(q[1]));
                    }
                } else {
                    let t = (x - p[0]) / (q[0] - p[0]);
                    if (0.0..=1.0).contains(&t) {
                        let y = p[1] + t * (q[1] - p[1]);
                        extent[0] = extent[0].min(y);
                        extent[1] = extent[1].max(y);
                    }
                }
            }
            if extent[0] <= extent[1] {
                intervals.push(extent);
            }
        }
    }
    intervals.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]));
    (crossings, intervals)
}

fn assert_command_resolution(plan: &WallPlan) -> [f64; 2] {
    let mut width_range = [f64::INFINITY, f64::NEG_INFINITY];
    for path in &plan.paths {
        let mut first = None;
        let mut prior: Option<(f64, f64)> = None;
        for edge in path.checked_edges().unwrap() {
            width_range[0] = width_range[0].min(edge.width_mm);
            width_range[1] = width_range[1].max(edge.width_mm);
            let length = (edge.points_mm[1][0] - edge.points_mm[0][0])
                .hypot(edge.points_mm[1][1] - edge.points_mm[0][1]);
            assert!(length >= program_coordinates::MIN_DEPOSITION_COMMAND_MM);
            if let Some((width, preceding_length)) = prior {
                assert!(
                    (edge.width_mm - width).abs()
                        <= program_coordinates::MAX_WALL_WIDTH_GRADIENT
                            * (length + preceding_length)
                            * 0.5
                );
            }
            first.get_or_insert((edge.width_mm, length));
            prior = Some((edge.width_mm, length));
        }
        if path.points_mm.first() == path.points_mm.last() {
            let (first_width, first_length) = first.unwrap();
            let (last_width, last_length) = prior.unwrap();
            assert!(
                (first_width - last_width).abs()
                    <= program_coordinates::MAX_WALL_WIDTH_GRADIENT
                        * (first_length + last_length)
                        * 0.5
            );
        }
    }
    width_range
}

#[test]
fn ordinary_rectangle_is_closed_and_retains_actual_residual() {
    let section = MultiPolygon(vec![polygon![
        (x: 0.0, y: 0.0), (x: 20.0, y: 0.0), (x: 20.0, y: 12.0), (x: 0.0, y: 12.0),
    ]]);
    let plan = plan(&section);
    assert!(
        plan.paths
            .iter()
            .all(|path| path.points_mm.first() == path.points_mm.last())
    );
    assert!(plan.fill.unsigned_area() > 0.0 && plan.fill.unsigned_area() <= 18.2 * 10.2);
    // Residual ownership excludes the commanded outer walls, rather than
    // returning the independent nominal contour-depth erosion unchanged.
    assert!(plan.fill.0.iter().all(|polygon| {
        polygon
            .exterior()
            .0
            .iter()
            .all(|p| p.x >= 0.9 && p.x <= 19.1 && p.y >= 0.9 && p.y <= 11.1)
    }));
    assert!(
        plan.paths
            .iter()
            .all(|path| path.owner == Some(DepositionOwner::Wall))
    );
    let limits = DepositionLimits {
        min_bead_width_mm: 0.3,
        max_bead_width_mm: 0.75,
        min_bead_height_mm: 0.000001,
        max_bead_height_mm: 0.3,
    };
    assert_eq!(
        width_envelope(&limits, 0.1).unwrap(),
        width_envelope(&limits, 0.2).unwrap(),
        "identical effective XY envelopes do not depend on physical-height bit patterns"
    );
    let mut tighter = limits;
    tighter.max_bead_width_mm = 0.72;
    assert_ne!(
        width_envelope(&limits, 0.1).unwrap(),
        width_envelope(&tighter, 0.1).unwrap(),
        "a consumed width limit changes wall reuse identity"
    );
}

#[test]
fn wide_outer_corner_continues_to_preferred_straights_without_losing_material_or_holes() {
    for hole in [false, true] {
        let outer = polygon![
            (x: 0.0, y: 0.0), (x: 20.0, y: 0.0),
            (x: 20.0, y: 10.0), (x: 0.0, y: 10.0),
        ];
        let holes = if hole {
            vec![geo::LineString::from(vec![
                (8.0, 3.0),
                (8.0, 7.0),
                (12.0, 7.0),
                (12.0, 3.0),
                (8.0, 3.0),
            ])]
        } else {
            Vec::new()
        };
        let section = MultiPolygon(vec![Polygon::new(outer.exterior().clone(), holes)]);
        let recipe = crate::planner::tests::recipe(0.25);
        let required = region::RequiredMaterial::from_section(&section);
        // Same .6/.4 intent and full native required material as the skin and
        // native-section consumers; planning exercises all physical gates.
        let plan = crate::planner::tests::wall_plan(
            &section,
            &required,
            &recipe,
            region::point_count(&section),
            &AtomicBool::new(false),
        )
        .unwrap();
        let widths = assert_command_resolution(&plan);
        assert!(widths[0] >= recipe.layer_height_mm && widths[0] < 0.5);
        assert_eq!(widths[1], recipe.bead_width_mm);
        assert_section_coverage(
            cross_section(&plan, 0, 10.0)
                .1
                .into_iter()
                .filter(|interval| interval[1] < 1.0)
                .collect(),
            [0.0, 0.6],
        );
        for corner in [[0.0, 0.0], [20.0, 0.0], [20.0, 10.0], [0.0, 10.0]] {
            let missing_extent = plan
                .paths
                .iter()
                .flat_map(|path| path.checked_edges().unwrap())
                .map(|edge| {
                    let line = Line::new(
                        [edge.points_mm[0][0], edge.points_mm[0][1]],
                        [edge.points_mm[1][0], edge.points_mm[1][1]],
                    );
                    Euclidean.distance(&geo::Point::from(corner), &line) - edge.width_mm * 0.5
                        + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM
                })
                .fold(f64::INFINITY, f64::min);
            assert!(
                missing_extent < 0.1 - super::super::OFFSET_TOLERANCE_MM * 0.5,
                "guaranteed-inner actual corner occupancy exceeds the unchanged extent: {missing_extent}"
            );
        }
        for ring in section.0[0].interiors() {
            for boundary in ring.lines() {
                for edge in plan
                    .paths
                    .iter()
                    .flat_map(|path| path.checked_edges().unwrap())
                {
                    let line = Line::new(
                        [edge.points_mm[0][0], edge.points_mm[0][1]],
                        [edge.points_mm[1][0], edge.points_mm[1][1]],
                    );
                    assert!(
                        Euclidean.distance(&line, &boundary) + super::super::OFFSET_TOLERANCE_MM
                            >= edge.width_mm * 0.5
                                + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM
                    );
                }
            }
        }
    }
}

#[test]
fn corner_extent_cannot_reduce_bead_below_actual_physical_height() {
    let section = MultiPolygon(vec![polygon![
        (x: 0.0, y: 0.0), (x: 20.0, y: 0.0),
        (x: 20.0, y: 10.0), (x: 0.0, y: 10.0),
    ]]);
    let mut recipe = crate::planner::tests::recipe(0.25);
    recipe.layer_height_mm = 0.5;
    let required = region::RequiredMaterial::from_section(&section);
    let failure = crate::planner::tests::wall_plan(
        &section,
        &required,
        &recipe,
        region::point_count(&section),
        &AtomicBool::new(false),
    )
    .err()
    .unwrap();
    assert_eq!(failure.code, ManufacturingErrorCode::UnsupportedGeometry);
}

#[test]
fn rounded_bulk_contours_do_not_become_false_thin_wall_arms() {
    let mut ring = Vec::new();
    for (center, start) in [
        ([28.0, 2.0], -std::f64::consts::FRAC_PI_2),
        ([28.0, 20.0], 0.0),
        ([2.0, 20.0], std::f64::consts::FRAC_PI_2),
        ([2.0, 2.0], std::f64::consts::PI),
    ] {
        for step in 0..=32 {
            let angle = start + std::f64::consts::FRAC_PI_2 * step as f64 / 32.0;
            ring.push(geo::Coord {
                x: center[0] + 2.0 * angle.cos(),
                y: center[1] + 2.0 * angle.sin(),
            });
        }
    }
    ring.push(ring[0]);
    let section = MultiPolygon(vec![Polygon::new(geo::LineString(ring), Vec::new())]);
    let plan = plan(&section);
    // Contained curved fits can spend radius; retain the actual contour-depth
    // and physical skin coverage contract, not constant command-width equality.
    assert_command_resolution(&plan);
    let (crossings, intervals) = cross_section(&plan, 0, 15.0);
    assert_eq!(
        crossings, 4,
        "two contour-depth needs remain on each broad physical side"
    );
    assert_section_coverage(
        intervals
            .iter()
            .copied()
            .filter(|interval| interval[1] < 11.0)
            .collect(),
        [0.0, 0.9],
    );
    assert_section_coverage(
        intervals
            .into_iter()
            .filter(|interval| interval[0] > 11.0)
            .collect(),
        [21.1, 22.0],
    );
}

#[test]
fn connected_t_keeps_all_required_arm_spines_in_actual_beads() {
    let section = MultiPolygon(vec![polygon![
        (x: 0.0, y: 0.0), (x: 20.0, y: 0.0), (x: 20.0, y: 0.6),
        (x: 10.3, y: 0.6), (x: 10.3, y: 12.0), (x: 9.7, y: 12.0),
        (x: 9.7, y: 0.6), (x: 0.0, y: 0.6),
    ]]);
    let plan = plan(&section);
    let mut anchors = crate::planner::support::AnchorWorkspace::default();
    for ends in [
        [[1.0, 0.3, 0.2], [19.0, 0.3, 0.2]],
        [[10.0, 0.3, 0.2], [10.0, 11.0, 0.2]],
    ] {
        let gaps = anchors
            .gaps(
                crate::planner::support::AnchorCandidate {
                    endpoints_mm: ends,
                    bead_width_mm: 0.3,
                },
                plan.paths.iter(),
                std::iter::empty(),
                &mut 0,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert!(
            gaps.is_empty(),
            "required arm spine has an uncovered interval: {gaps:?}"
        );
    }
}

fn assert_section_coverage(intervals: Vec<[f64; 2]>, target: [f64; 2]) {
    let mut cursor = target[0];
    let mut uncovered = 0.0;
    let mut largest_gap = 0.0f64;
    for [lo, hi] in intervals {
        assert!(
            lo >= target[0] - 0.005 && hi <= target[1] + 0.005,
            "no outward cross-section permission: interval={lo}..{hi}, target={target:?}"
        );
        if hi <= cursor {
            continue;
        }
        let gap = (lo.min(target[1]) - cursor).max(0.0);
        uncovered += gap;
        largest_gap = largest_gap.max(gap);
        cursor = cursor.max(hi.min(target[1]));
    }
    let final_gap = (target[1] - cursor).max(0.0);
    uncovered += final_gap;
    largest_gap = largest_gap.max(final_gap);
    assert!(
        uncovered <= (target[1] - target[0]) * 0.02 && largest_gap <= 0.005,
        "actual target material is missing: uncovered={uncovered}, largest_gap={largest_gap}"
    );
}

fn assert_connected_inner_material(plan: &WallPlan) {
    let edges: Vec<_> = plan
        .paths
        .iter()
        .map(|path| path.checked_edges().unwrap())
        .collect();
    assert!(!edges.is_empty());
    let mut connected = vec![false; edges.len()];
    connected[0] = true;
    loop {
        let mut changed = false;
        for from in 0..edges.len() {
            if !connected[from] {
                continue;
            }
            for to in 0..edges.len() {
                if connected[to] {
                    continue;
                }
                let joins = edges[from].clone().any(|a| {
                    edges[to].clone().any(|b| {
                        let line = |edge: DepositionEdge<'_>| {
                            Line::new(
                                [edge.points_mm[0][0], edge.points_mm[0][1]],
                                [edge.points_mm[1][0], edge.points_mm[1][1]],
                            )
                        };
                        let reach = (a.width_mm + b.width_mm) * 0.5
                            - 2.0 * program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM;
                        Euclidean.distance(&line(a), &line(b)) < reach
                    })
                });
                if joins {
                    connected[to] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    assert!(
        connected.iter().all(|value| *value),
        "actual guaranteed-inner path tubes must connect every required arm, not merely pass global area accounting: {connected:?}"
    );
}

#[test]
fn useful_taper_has_consumed_edge_widths_and_finite_continuing_gradient() {
    let section = MultiPolygon(vec![polygon![
        (x: 0.0, y: -0.275), (x: 22.0, y: -0.6),
        (x: 22.0, y: 0.6), (x: 0.0, y: 0.275),
    ]]);
    let plan = plan(&section);
    let width_range = assert_command_resolution(&plan);
    assert!(
        width_range[1] - width_range[0] >= 0.1,
        "actual admitted commands must use a useful width variation"
    );
    let mut counts = [0; 2];
    for (index, x) in [0.8, 20.0].into_iter().enumerate() {
        let target_width = 0.55 + x * 0.65 / 22.0;
        let target = [-target_width * 0.5, target_width * 0.5];
        let (count, intervals) = cross_section(&plan, 0, x);
        counts[index] = count;
        assert_section_coverage(intervals, target);
    }
    assert!(
        counts[0] > 0 && counts[1] > counts[0],
        "the wider physical cross-section needs a genuine additional bead, not extra Vec paths: {counts:?}"
    );
}

#[test]
fn short_arm_chooses_geometry_admitted_field_instead_of_unfit_objective_birth() {
    // Equal preferred-width departure for counts 3 and 4 occurs at T=10.8/7.
    // The nearly square original section has a ~.2-mm medial arm crossing that
    // objective, too short for the ~.7-mm 3->4 ramp. The admitted whole-arm
    // count/width field and any finite source-joint completion must still cover
    // the original section: preferred-width departure alone cannot choose it.
    let middle = 10.8 / 7.0;
    let section = MultiPolygon(vec![polygon![
        (x: 0.0, y: 0.0), (x: 1.74, y: 0.0),
        (x: 1.74, y: middle + 0.01), (x: 0.0, y: middle - 0.01),
    ]]);
    let plan = plan(&section);
    assert_command_resolution(&plan);
    let mut prior_count = None;
    for x in [0.84, 0.90] {
        let thickness = middle - 0.01 + x * 0.02 / 1.74;
        let (count, intervals) = cross_section(&plan, 0, x);
        assert!(
            (3..=4).contains(&count),
            "an admitted physical count, not a count jump: {count}"
        );
        if let Some(prior) = prior_count {
            assert_eq!(count, prior);
        }
        prior_count = Some(count);
        for edge in plan
            .paths
            .iter()
            .flat_map(|path| path.checked_edges().unwrap())
        {
            let [a, b] = [edge.points_mm[0][0], edge.points_mm[1][0]];
            if (a <= x && x < b) || (b <= x && x < a) {
                assert!(
                    (edge.width_mm - thickness / count as f64).abs() <= 2.0 * FIT_ERROR_MM,
                    "actual consumed T/n width, not a fixed-width substitute: {}",
                    edge.width_mm
                );
            }
        }
        assert_section_coverage(intervals, [0.0, thickness]);
    }
}

#[test]
fn junction_neighborhood_count_transition_preserves_incoming_field_and_actual_lane_gradient() {
    let section = MultiPolygon(vec![polygon![
        (x: 0.0, y: 0.0), (x: 20.0, y: 0.0), (x: 20.0, y: 0.6),
        (x: 10.305, y: 0.6), (x: 10.275, y: 10.6),
        (x: 9.725, y: 10.6), (x: 9.695, y: 0.6), (x: 0.0, y: 0.6),
    ]]);
    let plan = plan(&section);
    assert_command_resolution(&plan);
    let near = cross_section(&plan, 1, 2.0).0;
    let far = cross_section(&plan, 1, 8.0).0;
    assert!(
        near > far && far > 0,
        "actual junction-adjacent count change must remain useful: {near}/{far}"
    );
    for y in [0.72, 2.0, 8.0] {
        let width = 0.61 - (y - 0.6) * 0.006;
        assert_section_coverage(
            cross_section(&plan, 1, y).1,
            [10.0 - width * 0.5, 10.0 + width * 0.5],
        );
    }
    assert_connected_inner_material(&plan);
}
