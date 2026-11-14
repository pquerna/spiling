// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use geo::{Coord, Distance, Euclidean, Line, polygon};

fn plan(section: &MultiPolygon<f64>) -> WallPlan {
    plan_with(section, false)
}

fn plan_with(section: &MultiPolygon<f64>, arachne: bool) -> WallPlan {
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
    let generator = if arachne {
        plan_section_walls
    } else {
        plan_material_domain_walls
    };
    generator(
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
fn shallow_source_steps_keep_boundary_lanes_and_physically_admitted_corner_caps() {
    // The non-grid two-box union has short convex/reentrant steps. Collapsing a
    // curved source arm to its narrowest constant bead loses the requested band;
    // deleting its short exterior proposal does not repair that material loss.
    let boundary = [
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 0.234567891],
        [30.123456789, 0.234567891],
        [30.123456789, 10.234567891],
        [10.123456789, 10.234567891],
        [10.123456789, 10.0],
        [0.0, 10.0],
    ];
    for (height, minimum_width, maximum_width, reverse, offset) in [
        (0.4, 0.3, 1.0, false, [0.0, 0.0]),
        (0.4, 0.3, 1.0, true, [120.125, -30.375]),
        // The low machine width floor does not override actual bead height.
        // Source-field packing can fall below that height at the shallow step;
        // it must yield to the independently qualified boundary construction.
        (0.20000000000000007, 0.01, 2.0, false, [0.0, 0.0]),
        (0.20000000000000007, 0.01, 2.0, true, [120.125, -30.375]),
    ] {
        let mut points: Vec<_> = boundary
            .into_iter()
            .map(|p| Coord {
                x: p[0] + offset[0],
                y: p[1] + offset[1],
            })
            .collect();
        if reverse {
            points.reverse();
        }
        points.push(points[0]);
        let section = MultiPolygon(vec![Polygon::new(geo::LineString(points), vec![])]);
        let required = region::RequiredMaterial::from_section(&section);
        let mut recipe = crate::planner::tests::recipe(1.0);
        recipe.layer_height_mm = height;
        let limits = DepositionLimits {
            min_bead_width_mm: minimum_width,
            max_bead_width_mm: maximum_width,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.6,
        };
        let plan = plan_material_domain_walls(
            WallRequest {
                material: &section,
                required: &required,
                recipe: &recipe,
                limits: &limits,
                nozzle_diameter_mm: 0.4,
                lower_z: 0.0,
                upper_z: height,
                retained_points: region::point_count(&section),
                owned_bytes: 0,
            },
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        let [minimum, maximum] = assert_command_resolution(&plan);
        assert!(minimum >= recipe.layer_height_mm);
        assert!(maximum <= recipe.bead_width_mm);
        assert_connected_inner_material(&plan);
    }
}

/// Wall-generator census on real parts: every 0.2 mm layer of each STEP in
/// `SPILING_WALL_CORPUS`, sectioned as the tolerant path does, through the
/// variable-width generator. Prints the refusing gate per layer, tallied.
/// `cargo test -p spiling-manufacturing --profile profiling wall_census -- --ignored --nocapture`
#[test]
#[ignore = "needs SPILING_WALL_CORPUS with STEP files"]
fn wall_census() {
    use std::collections::BTreeMap;
    let dir = std::env::var("SPILING_WALL_CORPUS").expect("SPILING_WALL_CORPUS");
    let only = std::env::var("SPILING_WALL_PARTS").ok();
    let cancel = AtomicBool::new(false);
    let mut recipe = crate::planner::tests::recipe(0.15);
    recipe.bead_width_mm = 0.45;
    recipe.layer_height_mm = 0.2;
    recipe.perimeter_count = 2;
    let limits = DepositionLimits {
        min_bead_width_mm: 0.30,
        max_bead_width_mm: 0.75,
        min_bead_height_mm: 0.000001,
        max_bead_height_mm: 0.30,
    };
    let normalize = |message: &str| {
        let cut = message
            .find(|c: char| c == ':' || c == '[' || c.is_ascii_digit())
            .unwrap_or(message.len());
        message[..cut].trim().to_string()
    };
    let mut gates: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let (mut total, mut passed) = (0usize, 0usize);
    let mut incomplete_sources = 0usize;
    let mut failed_sections = 0usize;
    let mut planning_total = 0.0f64;
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if only
            .as_ref()
            .is_some_and(|o| !o.split(',').any(|p| name.starts_with(p)))
        {
            continue;
        }
        // Every placed solid, as the engine imports multi-body sources; each
        // layer's material is the union of their sections.
        let bytes = std::fs::read(&path).unwrap();
        let bodies = match spiling_geometry::source_bodies(&bytes, &cancel) {
            Ok(bodies) => bodies,
            Err(error) => {
                println!("REFUSE_SOURCE {name}: {error:?}");
                incomplete_sources += 1;
                continue;
            }
        };
        let mut models = Vec::new();
        let expected_models = bodies
            .iter()
            .map(|body| body.placements.len())
            .sum::<usize>();
        for body in &bodies {
            let Ok(definition) =
                spiling_geometry::from_step_solid_with_cancel(&bytes, &body.solid, &cancel)
            else {
                continue;
            };
            let Ok(model) = spiling_geometry::tolerant::TolerantModel::build(
                &definition,
                Default::default(),
                &cancel,
            ) else {
                continue;
            };
            for pose in &body.placements {
                models.push(model.placed(pose));
            }
        }
        if models.len() != expected_models || models.is_empty() {
            incomplete_sources += 1;
            println!(
                "SKIP {name}: incomplete source preparation, {}/{} placed solids",
                models.len(),
                expected_models,
            );
            continue;
        }
        println!(
            "SOURCE {name}: {}/{} placed solids",
            models.len(),
            expected_models
        );
        let bounds = models.iter().map(|m| m.bounds()).fold(
            ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
            |(lo, hi), (a, b)| {
                (
                    std::array::from_fn(|i| lo[i].min(a[i])),
                    std::array::from_fn(|i| hi[i].max(b[i])),
                )
            },
        );
        let (min, max) = bounds;
        let (mut part_total, mut part_passed) = (0usize, 0usize);
        let mut part_planning = 0.0f64;
        let mut digest = std::collections::hash_map::DefaultHasher::new();
        let started = std::time::Instant::now();
        // Sample at most 60 layers evenly so one tall part cannot dominate.
        let layers = ((max[2] - min[2]) / 0.2).floor().max(1.0);
        let step = 0.2 * (layers / 60.0).ceil().max(1.0);
        let mut z = min[2] + 0.1;
        while z < max[2] {
            let mut material = MultiPolygon(Vec::new());
            let mut failed = false;
            for model in &models {
                let Ok(section) = model.section(z) else {
                    failed = true;
                    break;
                };
                let piece =
                    geo::Simplify::simplify(&super::super::tolerant::polygons(&section), 0.005);
                material = if material.0.is_empty() {
                    piece
                } else {
                    geo::BooleanOps::union(&material, &piece)
                };
            }
            if failed {
                println!("REFUSE_SECTION {name} z={z:.2}");
                failed_sections += 1;
                part_total += 1;
                z += step;
                continue;
            }
            if models.len() > 1 {
                // As the tolerant path fuses touching solids.
                let admit =
                    |_: usize, _: usize, _: Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>| Ok(());
                let grown = region::offset_admitted(&material, 0.0025, admit, &cancel).unwrap();
                material = region::offset_admitted(&grown, -0.0025, admit, &cancel).unwrap();
            }
            region::canonicalize(&mut material);
            if material.0.is_empty() {
                z += step;
                continue;
            }
            part_total += 1;
            let required = region::RequiredMaterial::from_section(&material);
            let planning = std::time::Instant::now();
            let generate = if std::env::var("SPILING_WALL_GENERATOR").as_deref() == Ok("arachne") {
                plan_section_walls
            } else {
                plan_material_domain_walls
            };
            let result = generate(
                WallRequest {
                    material: &material,
                    required: &required,
                    recipe: &recipe,
                    limits: &limits,
                    nozzle_diameter_mm: 0.4,
                    owned_bytes: 0,
                    lower_z: z - 0.1,
                    upper_z: z + 0.1,
                    retained_points: region::point_count(&material),
                },
                &mut 0,
                &cancel,
            );
            part_planning += planning.elapsed().as_secs_f64();
            match result {
                Ok(plan) => {
                    part_passed += 1;
                    use std::hash::Hash;
                    for path in &plan.paths {
                        for p in &path.points_mm {
                            p.map(f64::to_bits).hash(&mut digest);
                        }
                        format!("{:?}", path.width).hash(&mut digest);
                    }
                    for ring in geo::BooleanOps::rings(&plan.fill) {
                        for c in &ring.0 {
                            [c.x.to_bits(), c.y.to_bits()].hash(&mut digest);
                        }
                    }
                }
                Err(e) => {
                    println!(
                        "REFUSE {name} z={z:.2} {:?}: {}",
                        e.code,
                        &e.message[..e.message.len().min(
                            if std::env::var_os("SPILING_WALL_FULL").is_some() {
                                4000
                            } else {
                                160
                            }
                        )]
                    );
                    let entry = gates
                        .entry(format!("{:?}: {}", e.code, normalize(&e.message)))
                        .or_default();
                    entry.0 += 1;
                    if entry.1.len() < 3 {
                        entry.1.push(format!(
                            "{name} z={z:.2}: {}",
                            &e.message[..e.message.len().min(300)]
                        ));
                    }
                }
            }
            z += step;
        }
        println!(
            "PART {name}: {part_passed}/{part_total} layers in {:.1} s (walls {part_planning:.2} s) plans {:016x}",
            started.elapsed().as_secs_f64(),
            std::hash::Hasher::finish(&digest)
        );
        planning_total += part_planning;
        total += part_total;
        passed += part_passed;
    }
    println!("\nALL: {passed}/{total} layers pass, walls {planning_total:.2} s");
    let mut sorted: Vec<_> = gates.into_iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.1.0));
    for (gate, (count, examples)) in sorted {
        println!("\n{count:6}  {gate}");
        for example in examples {
            println!("        {example}");
        }
    }
    assert_eq!(
        incomplete_sources, 0,
        "wall census omitted complete sources"
    );
    assert_eq!(failed_sections, 0, "wall census omitted requested sections");
}

#[test]
fn classic_walls_repeat_bit_identically_on_near_degenerate_circles() {
    // With OS-seeded skip-list levels in boostvoronoi's beach line about a
    // third of these constructions produced a different medial graph.
    let section = near_degenerate_circles();
    let mut recipe = crate::planner::tests::recipe(0.25);
    recipe.bead_width_mm = 0.45;
    recipe.layer_height_mm = 0.2;
    recipe.perimeter_count = 2;
    let limits = DepositionLimits {
        min_bead_width_mm: 0.30,
        max_bead_width_mm: 0.75,
        min_bead_height_mm: 0.0025,
        max_bead_height_mm: 0.30,
    };
    let required = region::RequiredMaterial::from_section(&section);
    let plan = || {
        let plan = plan_section_walls(
            WallRequest {
                material: &section,
                required: &required,
                recipe: &recipe,
                limits: &limits,
                nozzle_diameter_mm: 0.4,
                owned_bytes: 0,
                lower_z: 1.9,
                upper_z: 2.1,
                retained_points: region::point_count(&section),
            },
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        format!("{:?} {:?}", plan.paths, plan.fill)
    };
    let first = plan();
    for run in 1..24 {
        assert!(plan() == first, "wall plan {run} differs from the first");
    }
}

/// Two r = 3 mm tessellated holes' material as a sliced STEP part presents
/// them: 128 vertices each, quadrant vertices displaced by about 1e-7 mm.
/// The near-cocircular sites make segment-Voronoi beach-line searches
/// sensitive to the skip list's tower heights.
fn near_degenerate_circles() -> MultiPolygon<f64> {
    const OCTANT: [[f64; 2]; 17] = [
        [-2.999999999999986, -1.1559272650174535e-07],
        [-2.99696464576094, -0.13491816800912204],
        [-2.987864714907573, -0.26956343485394996],
        [-2.9721035123586717, -0.40816750461692664],
        [-2.9499160833894535, -0.5458892753665339],
        [-2.920404180252845, -0.6864688077120462],
        [-2.8841968438245402, -0.8254747519291215],
        [-2.8402041324484344, -0.9660437288357002],
        [-2.789364867412772, -1.1042842190501858],
        [-2.7305374596595016, -1.242644430799146],
        [-2.6648519848607037, -1.377883848074248],
        [-2.591277047003004, -1.511715338837078],
        [-2.5109864409061657, -1.6416294020226587],
        [-2.423230222686769, -1.7686026370718082],
        [-2.32905947244015, -1.8908944904030704],
        [-2.228170621289408, -2.008794584426866],
        [-2.1213203435596313, -2.1213203435596313],
    ];
    let mut quarter: Vec<[f64; 2]> = OCTANT.to_vec();
    quarter.extend(OCTANT[1..16].iter().rev().map(|p| [p[1], p[0]]));
    let ring = |c: [f64; 2]| {
        let mut points = Vec::new();
        for turn in 0..4 {
            for p in &quarter {
                let r = match turn {
                    0 => *p,
                    1 => [-p[1], p[0]],
                    2 => [-p[0], -p[1]],
                    _ => [p[1], -p[0]],
                };
                points.push(Coord {
                    x: c[0] + r[0],
                    y: c[1] + r[1],
                });
            }
        }
        points.push(points[0]);
        Polygon::new(geo::LineString(points), Vec::new())
    };
    MultiPolygon(vec![ring([92.9765, 110.353]), ring([127.7295, 110.353])])
}

#[test]
fn thin_features_widen_to_the_narrowest_bead_or_are_reported_omitted() {
    let strip = |width: f64| {
        MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0),
            (x: 20.0, y: 0.0),
            (x: 20.0, y: width),
            (x: 0.0, y: width),
        ]])
    };
    // Narrower than the narrowest bead, wider than a quarter nozzle.
    let widened = plan_with(&strip(0.25), true);
    assert_eq!(widened.paths.len(), 1);
    assert!(
        widened.thin_widened_mm > 19.0,
        "{}",
        widened.thin_widened_mm
    );
    assert_eq!(widened.thin_omitted_area_mm2, 0.0);
    // Thinner than a quarter nozzle: nothing printed, the area reported.
    let omitted = plan_with(&strip(0.05), true);
    assert!(omitted.paths.is_empty());
    assert!(omitted.fill.0.is_empty());
    assert!(
        (omitted.thin_omitted_area_mm2 - 1.0).abs() < 0.05,
        "{}",
        omitted.thin_omitted_area_mm2
    );
}
