// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::{SectionLoopMm, import_step, section};
use std::{f64::consts::PI, path::Path, sync::atomic::AtomicBool};

fn fixture(name: &str) -> Definition {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry")
        .join(name);
    let bytes = std::fs::read(path).expect("frozen original geometry fixture");
    import_step(&bytes, &AtomicBool::new(false)).expect("admitted original native fixture")
}

fn signed_area(boundary: &SectionLoopMm, frame: &PlaneFrameMm) -> f64 {
    let origin = in_frame(boundary.points_mm[0], frame).unwrap();
    boundary
        .points_mm
        .windows(2)
        .map(|edge| {
            let a = in_frame(edge[0], frame).unwrap();
            let b = in_frame(edge[1], frame).unwrap();
            ((a[0] - origin[0]) * (b[1] - origin[1]) - (b[0] - origin[0]) * (a[1] - origin[1]))
                / 2.0
        })
        .sum()
}

fn assert_canonical(result: &NativeSection) {
    assert!(result.achieved_chord_error_mm().is_finite());
    assert!(result.achieved_plane_error_mm().is_finite());
    assert!((0.0..=SECTION_SAMPLING_TOLERANCE_MM).contains(&result.achieved_chord_error_mm()));
    assert!((0.0..=SECTION_PLANE_TOLERANCE_MM).contains(&result.achieved_plane_error_mm()));
    let mut seen_hole = false;
    for boundary in &result.loops {
        assert!(boundary.points_mm.len() >= 4);
        assert_eq!(boundary.points_mm.first(), boundary.points_mm.last());
        let area = signed_area(boundary, &result.frame);
        assert!(if boundary.is_hole {
            area < 0.0
        } else {
            area > 0.0
        });
        seen_hole |= boundary.is_hole;
        assert!(
            !seen_hole || boundary.is_hole,
            "all outer loops precede holes"
        );
        let first = in_frame(boundary.points_mm[0], &result.frame).unwrap();
        for point in &boundary.points_mm {
            let local = in_frame(*point, &result.frame).unwrap();
            assert!(local[2].abs() <= SECTION_PLANE_TOLERANCE_MM);
            assert!(
                first <= local,
                "canonical first point is lexically least in plane frame"
            );
        }
    }
}

fn parity_definition(name: &str) -> Definition {
    let (_, solid, _) = crate::parity_fixtures::recipes()
        .into_iter()
        .find(|(candidate, _, _)| *candidate == name)
        .expect("original native parity construction");
    let source = crate::parity_fixtures::original(name, solid);
    import_step(source.as_bytes(), &AtomicBool::new(false))
        .expect("bounded source-backed native parity definition")
}

#[test]
fn original_trim_queries_bind_the_immutable_definition_and_face() {
    let definition = fixture("box-mm.step");
    let other = fixture("box-mm.step");
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(20_000_000);
    let originals = crate::parametric::prepare_section_originals(
        &definition,
        MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap()
    .expect("source-backed original trim domains");
    let source = OriginalTrimSource {
        definition: &definition,
        originals: Some(&originals),
        vertex_plans: None,
    };
    let wrong = OriginalTrimSource {
        definition: &other,
        originals: Some(&originals),
        vertex_plans: None,
    };
    for (index, identity) in definition.face_boundaries.iter().enumerate() {
        let domain = source
            .face_domain(index, MAX_QUERY_SEED_BYTES, &mut work, &cancel)
            .unwrap();
        assert_eq!(domain.native().face().id(), identity.face);
        assert_eq!(domain.owned_bytes(), 0);
        // Equal imported geometry is not authority to borrow another instance's proof.
        assert!(
            wrong
                .face_domain(index, MAX_QUERY_SEED_BYTES, &mut work, &cancel)
                .is_err()
        );
    }
    assert!(
        source
            .face_domain(
                definition.face_boundaries.len(),
                MAX_QUERY_SEED_BYTES,
                &mut work,
                &cancel,
            )
            .is_err()
    );
    assert!(
        source
            .face_domain(0, usize::MAX, &mut work, &cancel,)
            .is_err(),
        "metadata-inclusive scratch overflow must refuse"
    );
    assert!(
        source
            .face_domain(0, MAX_QUERY_SEED_BYTES, &mut work, &AtomicBool::new(true),)
            .is_err(),
        "borrowing still observes cancellation"
    );
}

#[test]
fn original_and_retained_implicit_box_sections_preserve_physical_material() {
    let cancel = AtomicBool::new(false);
    let implicit = crate::corpus_fixtures::implicit_intersections(
        include_str!("../../../../fixtures/geometry/box-mm.step"),
        "implicit-box.step",
    );
    for (name, definition) in [
        ("original box", fixture("box-mm.step")),
        (
            "original implicit box",
            import_step(implicit.as_bytes(), &cancel).unwrap(),
        ),
    ] {
        let mut work = GeometryWorkBudget::new(20_000_000);
        let source = prepare_section_source(
            &definition,
            MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        for normal in [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0]] {
            let plane = PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal,
            };
            let result = source
                .section_with_byte_limit(
                    plane,
                    &cancel,
                    spiling_contracts::geometry::MAX_SECTION_BYTES,
                    &mut work,
                )
                .unwrap_or_else(|error| panic!("{name}: {error:?}"));
            assert_eq!(result.loops.len(), 1, "{name}");
            assert_canonical(&result);
            assert!(!result.loops[0].is_hole, "{name}");
            assert!(
                (signed_area(&result.loops[0], &result.frame) - 200.0).abs() < 1e-7,
                "{name}: native Boolean must retain the complete 20 by 10 mm material"
            );
            assert_eq!(
                result.loop_source_face_ordinals(0).unwrap().len(),
                4,
                "{name}: all four original lateral faces own the native section"
            );
        }
    }
}

#[test]
fn unchanged_independent_fitting_section_preserves_taper_and_bore() {
    let definition = fixture("independent-exporters/occt-tapered-fitting-mm.step");
    let result = section(
        &definition,
        PlaneMm {
            origin_mm: [0.0, 0.0, 6.0],
            normal: [0.0, 0.0, 1.0],
        },
        &AtomicBool::new(false),
    )
    .expect("unchanged original OCCT fitting uses the certified native Boolean");
    assert_canonical(&result);
    assert_eq!(result.loops.len(), 2);
    assert!(!result.loops[0].is_hole);
    assert!(result.loops[1].is_hole);
    let area: f64 = result
        .loops
        .iter()
        .map(|boundary| signed_area(boundary, &result.frame))
        .sum();
    // At z=6 the original cone has radius five and the bore radius two.
    // Both native circle/polygon discrepancies retain the frozen chord budget.
    let error = 14.0 * PI * SECTION_SAMPLING_TOLERANCE_MM
        + 2.0 * PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2)
        + 1e-7;
    assert!((area - 21.0 * PI).abs() <= error, "area={area}");
    assert!((0..result.loops.len()).all(|index| {
        result
            .loop_source_face_ordinals(index)
            .is_some_and(|owners| !owners.is_empty())
    }));
}

#[test]
fn unchanged_fitting_near_cap_section_preserves_original_back_root_and_material() {
    let definition = fixture("independent-exporters/occt-tapered-fitting-mm.step");
    let height = 0.0999999912280684;
    let result = section(
        &definition,
        PlaneMm {
            // Exact source-frame plane of the centered operator occurrence.
            origin_mm: [-110.0, -110.0, height],
            normal: [0.0, 0.0, 1.0],
        },
        &AtomicBool::new(false),
    )
    .expect("back-only original endpoint seeds its unchanged negative covering chart");
    assert_canonical(&result);
    assert_eq!(result.loops.len(), 2);
    assert!(!result.loops[0].is_hole);
    assert!(result.loops[1].is_hole);
    let outer_radius = 6.0 - height / 6.0;
    let expected = PI * (outer_radius * outer_radius - 4.0);
    let area: f64 = result
        .loops
        .iter()
        .map(|boundary| signed_area(boundary, &result.frame))
        .sum();
    let error = 2.0 * PI * (outer_radius + 2.0) * SECTION_SAMPLING_TOLERANCE_MM
        + 2.0 * PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2)
        + 1e-7;
    assert!(
        (area - expected).abs() <= error,
        "area={area}, expected={expected}"
    );
    assert!((0..result.loops.len()).all(|index| {
        result
            .loop_source_face_ordinals(index)
            .is_some_and(|owners| !owners.is_empty())
    }));
}

#[test]
fn unchanged_housing_near_floor_preserves_trimmed_material_under_plane_reversal() {
    let definition = fixture("independent-exporters/occt-rounded-housing-mm.step");
    let cancel = AtomicBool::new(false);
    let mut preparation_work = GeometryWorkBudget::new(20_000_000);
    let prepared = prepare_section_source(
        &definition,
        MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut preparation_work,
    )
    .unwrap();
    let query_bytes =
        u32::try_from(MAX_PREPARED_SECTION_BYTES - prepared.retained_bytes()).unwrap();
    for height in [0.0125_f64, 0.4249999535616466] {
        // The original radius-two lower fillets offset the 20-by-14 core rectangle.
        // This source-derived area oracle is independent of the native guide order.
        let radius = (4.0 - (2.0 - height).powi(2)).sqrt();
        let expected = 20.0 * 14.0 + 2.0 * radius * (20.0 + 14.0) + PI * radius * radius;
        let perimeter = 2.0 * (20.0 + 14.0) + 2.0 * PI * radius;
        let error = perimeter * SECTION_SAMPLING_TOLERANCE_MM
            + PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2)
            + 1e-7;
        for normal_z in [1.0, -1.0] {
            for origin_xy in [[0.0, 0.0], [-110.0, -110.0]] {
                let mut work = GeometryWorkBudget::new(20_000_000);
                let result = prepared
                    .section_with_byte_limit(
                        PlaneMm {
                            origin_mm: [origin_xy[0], origin_xy[1], height],
                            normal: [0.0, 0.0, normal_z],
                        },
                        &cancel,
                        query_bytes,
                        &mut work,
                    )
                    .expect(
                        "actual original trim contacts bound the fresh native intersection guide",
                    );
                assert_canonical(&result);
                assert_eq!(result.loops.len(), 1);
                assert!(!result.loops[0].is_hole);
                let area = signed_area(&result.loops[0], &result.frame);
                assert!(
                    (area - expected).abs() <= error,
                    "height={height}, normal_z={normal_z}, origin_xy={origin_xy:?}, area={area}, expected={expected}"
                );
                assert!(
                    result
                        .loop_source_face_ordinals(0)
                        .is_some_and(|owners| !owners.is_empty())
                );
            }
        }
    }
}

#[test]
fn native_curved_interior_sections_retain_material_and_torus_holes() {
    let cancel = AtomicBool::new(false);
    for (name, plane, expected_area, expected_loops) in [
        (
            "native-parity-sphere-belt.step",
            PlaneMm {
                origin_mm: [4.5, 0.0, 0.0],
                normal: [1.0, 0.0, 0.0],
            },
            4.75 * PI,
            1,
        ),
        (
            "native-parity-sphere-belt.step",
            PlaneMm {
                origin_mm: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
            },
            25.0 * PI,
            1,
        ),
        (
            "native-parity-full-sphere.step",
            PlaneMm {
                origin_mm: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
            },
            25.0 * PI,
            1,
        ),
        (
            "native-parity-full-sphere.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 1.0],
                normal: [0.0, 0.0, 1.0],
            },
            24.0 * PI,
            1,
        ),
        (
            "native-parity-ring-torus.step",
            PlaneMm {
                origin_mm: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
            },
            40.0 * PI,
            2,
        ),
        (
            "native-parity-cone-frustum.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
            16.0 * PI,
            1,
        ),
        (
            "native-parity-bspline-box.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
            200.0,
            1,
        ),
        (
            "native-parity-nurbs-box.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
            200.0,
            1,
        ),
        (
            "native-parity-bezier-extrusion.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
            // Integral of the cubic lower profile adds exactly 21 mm².
            221.0,
            1,
        ),
        (
            "native-parity-revolution.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
            12.0 * PI,
            2,
        ),
        (
            "native-parity-blind-pocket.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 2.0],
                normal: [0.0, 0.0, 1.0],
            },
            400.0,
            1,
        ),
        (
            "native-parity-blind-pocket.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 6.0],
                normal: [0.0, 0.0, 1.0],
            },
            400.0 - 9.0 * PI,
            2,
        ),
        (
            "native-parity-blind-pocket-nurbs-floor.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 2.0],
                normal: [0.0, 0.0, 1.0],
            },
            400.0,
            1,
        ),
        (
            "native-parity-blind-pocket-nurbs-floor.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 6.0],
                normal: [0.0, 0.0, 1.0],
            },
            400.0 - 9.0 * PI,
            2,
        ),
    ] {
        let definition = parity_definition(name);
        let source_faces: Vec<_> = definition
            .faces()
            .iter()
            .map(|face| face.face_id.clone())
            .collect();
        let mut work = GeometryWorkBudget::new(50_000_000);
        let prepared = prepare_section_source(
            &definition,
            MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let mut result = prepared
            .section_with_byte_limit(
                plane,
                &cancel,
                spiling_contracts::geometry::MAX_SECTION_BYTES,
                &mut work,
            )
            .expect("regular curved sections use native intersections");
        assert_canonical(&result);
        assert_eq!(result.loops.len(), expected_loops);
        let area: f64 = result
            .loops
            .iter()
            .map(|boundary| signed_area(boundary, &result.frame))
            .sum();
        assert!(
            (area - expected_area).abs() < 0.15,
            "{name}: {area} versus {expected_area}"
        );
        assert_eq!(
            result
                .loops
                .iter()
                .filter(|boundary| boundary.is_hole)
                .count(),
            expected_loops - 1,
        );
        for index in 0..result.loops.len() {
            let owners = result
                .loop_source_face_ordinals(index)
                .expect("actual native loops retain paired original source owners");
            assert!(!owners.is_empty());
            assert!(owners.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(owners.iter().all(|owner| *owner < definition.faces().len()));
        }
        let lineage_snapshot = |section: &NativeSection| {
            section
                .loops
                .iter()
                .enumerate()
                .map(|(index, boundary)| {
                    (
                        boundary.points_mm[0],
                        boundary.is_hole,
                        section.loop_source_face_ordinals(index).unwrap().to_vec(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let original_lineage = lineage_snapshot(&result);
        result.loops.reverse();
        result.loop_source_faces.reverse();
        canonicalize_section_in_place(
            &mut result,
            spiling_contracts::geometry::MAX_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_canonical(&result);
        assert_eq!(lineage_snapshot(&result), original_lineage);
        assert_eq!(
            definition
                .faces()
                .iter()
                .map(|face| face.face_id.clone())
                .collect::<Vec<_>>(),
            source_faces,
        );
    }
}

#[test]
fn curved_critical_planes_refuse_instead_of_returning_empty_regions() {
    let cancel = AtomicBool::new(false);
    for (name, plane) in [
        (
            "native-parity-sphere-belt.step",
            PlaneMm {
                origin_mm: [5.0, 0.0, 0.0],
                normal: [1.0, 0.0, 0.0],
            },
        ),
        (
            "native-parity-full-sphere.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 5.0],
                normal: [0.0, 0.0, 1.0],
            },
        ),
        (
            "native-parity-full-sphere.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, -5.0],
                normal: [0.0, 0.0, 1.0],
            },
        ),
        (
            "native-parity-ring-torus.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 2.0],
                normal: [0.0, 0.0, 1.0],
            },
        ),
        (
            "native-parity-blind-pocket.step",
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
        ),
    ] {
        assert_eq!(
            section(&parity_definition(name), plane, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::DegenerateSection,
        );
    }
    let sphere = parity_definition("native-parity-sphere-belt.step");
    assert!(
        section(
            &sphere,
            PlaneMm {
                origin_mm: [6.0, 0.0, 0.0],
                normal: [1.0, 0.0, 0.0]
            },
            &cancel,
        )
        .unwrap()
        .loops
        .is_empty()
    );
}

#[test]
fn nurbs_blind_pocket_floor_never_promotes_a_critical_plane_to_regular_regions() {
    let definition = parity_definition("native-parity-blind-pocket-nurbs-floor.step");
    let error = section(
        &definition,
        PlaneMm {
            origin_mm: [0.0, 0.0, 4.0],
            normal: [0.0, 0.0, 1.0],
        },
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(
        matches!(
            error.code,
            GeometryErrorCode::DegenerateSection | GeometryErrorCode::AnalysisIndeterminate,
        ),
        "critical existence must be proved or explicitly unresolved, not inferred from family"
    );
}

#[test]
fn omitted_interior_native_root_branch_is_not_a_successful_section_certificate() {
    let definition = parity_definition("native-parity-sphere-belt.step");
    let native_plane = monstertruck_modeling::Plane::new(
        Point3::new(4.5, 0.0, 0.0),
        Point3::new(4.5, 1.0, 0.0),
        Point3::new(4.5, 0.0, 1.0),
    );
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(1_000_000);
    let originals = crate::parametric::prepare_section_originals(
        &definition,
        loops::MAX_CURVE_CAPTURE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap()
    .unwrap();
    let roots = crate::parametric::prepare_section_plane_roots(
        &originals,
        &native_plane,
        loops::MAX_CURVE_CAPTURE_BYTES - originals.retained_bytes(),
        &mut work,
        &cancel,
    )
    .unwrap();
    let error = crate::parametric::certify_section_plane_coverage(
        &definition,
        &roots,
        &[],
        &[],
        loops::MAX_CURVE_CAPTURE_BYTES - originals.retained_bytes() - roots.retained_bytes(),
        &mut work,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
}

#[test]
fn exact_geometry_from_another_definition_cannot_borrow_original_source_roots() {
    let definition = fixture("box-mm.step");
    let foreign = fixture("box-mm.step");
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(1_000_000);
    let plane = monstertruck_modeling::Plane::new(
        Point3::new(0.0, 0.0, 100.0),
        Point3::new(1.0, 0.0, 100.0),
        Point3::new(0.0, 1.0, 100.0),
    );
    let originals = crate::parametric::prepare_section_originals(
        &definition,
        loops::MAX_CURVE_CAPTURE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap()
    .unwrap();
    let roots = crate::parametric::prepare_section_plane_roots(
        &originals,
        &plane,
        loops::MAX_CURVE_CAPTURE_BYTES - originals.retained_bytes(),
        &mut work,
        &cancel,
    )
    .unwrap();
    let remaining =
        loops::MAX_CURVE_CAPTURE_BYTES - originals.retained_bytes() - roots.retained_bytes();
    // This is genuinely empty geometry with no endpoint IDs to catch a stale
    // source by accident; only the original source capture may authorize it.
    crate::parametric::certify_section_plane_coverage(
        &definition,
        &roots,
        &[],
        &[],
        remaining,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        crate::parametric::certify_section_plane_coverage(
            &foreign,
            &roots,
            &[],
            &[],
            remaining,
            &mut work,
            &cancel,
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::InvalidGeometry
    );
}

#[test]
fn absent_hole_search_candidates_cannot_pass_whole_face_plane_coverage() {
    let definition = fixture("through-hole.step");
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(20_000_000);
    let source = prepare_section_source(
        &definition,
        MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut work,
    )
    .unwrap();
    let frame = PlaneFrameMm::from_plane(PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    })
    .unwrap();
    let (min, max) = clip_bounds(&definition, &frame).unwrap();
    let clip: Solid = primitive::cuboid(BoundingBox::from_iter([
        Point3::from(min),
        Point3::from(max),
    ]));
    let clip = builder::transformed(&clip, frame_to_definition(&frame));
    let (cap_id, native_plane) = clip
        .face_iter()
        .find_map(|face| {
            face.with_surface(|surface| {
                let Surface::Plane(plane) = surface else {
                    return None;
                };
                (parallel(plane.normal().into(), [0.0, 0.0, 1.0])
                    && (plane.origin().z - 4.0).abs() <= SECTION_PLANE_TOLERANCE_MM)
                    .then(|| (face.id(), *plane))
            })
        })
        .unwrap();
    let native_work = BooleanWorkBudget::new(10_000_000);
    let cut = monstertruck_solid::and_prepared_with_provenance_counted_with_query_bytes(
        &source.native,
        &clip,
        &cancel,
        &native_work,
        MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
    )
    .unwrap();
    crate::parametric::debit_native_work(&mut work, native_work.consumed(), &cancel).unwrap();
    let caps = cut
        .face_origins
        .iter()
        .filter_map(|origin| {
            (origin.source.operand == BooleanOperand::Other && origin.source.face_id == cap_id)
                .then_some(origin.output_face)
        })
        .collect::<HashSet<_>>();
    let captures = loops::CapturedCurves::collect(
        &definition,
        &cut,
        &caps,
        loops::MAX_CURVE_CAPTURE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let roots = crate::parametric::prepare_section_plane_roots(
        source.originals.as_ref().unwrap(),
        &native_plane,
        captures.remaining_bytes(),
        &mut work,
        &cancel,
    )
    .unwrap();
    let (mut curves, remaining) = captures
        .prepare(
            &definition,
            &native_plane,
            &roots,
            roots.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
    crate::parametric::certify_section_plane_coverage(
        &definition,
        &roots,
        &curves,
        captures.endpoint_origins(),
        remaining,
        &mut work,
        &cancel,
    )
    .unwrap();
    let complete_count = curves.len();
    // Deliberately simulate a search that found only exterior candidates.
    // This selection is test corruption, never a production geometry proof.
    let mut kept_candidates = Vec::new();
    let mut old_candidate_index = 0;
    curves.retain(|curve| {
        let (a, b) = curve.parameter_intervals().next().unwrap();
        let t = a + (b - a) * 0.5;
        let point = curve.jet((t, t), &mut work, &cancel).unwrap().position;
        let keep = (point[0].mid() - 10.0).hypot(point[1].mid() - 10.0) > 5.0;
        if keep {
            kept_candidates.push(old_candidate_index);
        }
        old_candidate_index += 1;
        keep
    });
    assert!(!curves.is_empty() && curves.len() < complete_count);
    // Keep the remaining genealogy valid: rejection must concern the missing
    // material branch, not a dangling candidate index from test corruption.
    let remaining_origins = captures
        .endpoint_origins()
        .iter()
        .filter_map(|origin| {
            kept_candidates
                .iter()
                .position(|index| *index == origin.candidate_index)
                .map(
                    |candidate_index| endpoint_genealogy::SectionPlaneEndpointOrigin {
                        candidate_index,
                        ..*origin
                    },
                )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        crate::parametric::certify_section_plane_coverage(
            &definition,
            &roots,
            &curves,
            &remaining_origins,
            remaining,
            &mut work,
            &cancel,
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::AnalysisIndeterminate
    );
}

#[test]
fn exact_fit_prepared_source_keeps_independent_clip_capacity_and_native_queries() {
    let definition = fixture("box-mm.step");
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(1_000_000);
    let baseline = prepare_section_source(
        &definition,
        MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut work,
    )
    .unwrap();
    let source_bytes = baseline.retained_bytes();
    let pinned_bytes = definition.retained_query_proof_bytes();
    drop(baseline);

    let insufficient = prepare_section_source(&definition, source_bytes - 1, &cancel, &mut work)
        .err()
        .expect("source capacity must reject one byte below complete retained preparation");
    assert_eq!(insufficient.code, GeometryErrorCode::ResourceLimit);
    let source = prepare_section_source(&definition, source_bytes, &cancel, &mut work).unwrap();
    assert_eq!(source.retained_bytes(), source_bytes);
    let proof_only = prepare_section_source(&definition, pinned_bytes, &cancel, &mut work)
        .err()
        .expect("pinned proofs cannot also fund native search seeds");
    assert_eq!(proof_only.code, GeometryErrorCode::ResourceLimit);
    for (z, normal) in [
        (4.0, [0.0, 0.0, 1.0]),
        (1.0, [0.0, 0.0, -1.0]),
        (7.0, [0.0, 0.0, 1.0]),
    ] {
        let mut result = source
            .section_with_byte_limit(
                PlaneMm {
                    origin_mm: [0.0, 0.0, z],
                    normal,
                },
                &cancel,
                spiling_contracts::geometry::MAX_SECTION_BYTES,
                &mut work,
            )
            .unwrap();
        assert_canonical(&result);
        let area: f64 = result
            .loops
            .iter()
            .map(|boundary| signed_area(boundary, &result.frame))
            .sum();
        assert!(
            (area - 200.0).abs() < 1e-7,
            "unchanged analytic box must remain whole across prepared cuts"
        );
        let native_loop_buffer = result.loops.as_ptr();
        let native_point_buffer = result.loops[0].points_mm.as_ptr();
        let retained = u32::try_from(result.retained_bytes()).unwrap();
        assert_eq!(
            canonicalize_section_in_place(&mut result, retained, &mut work, &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_canonical(&result);
        canonicalize_section_in_place(
            &mut result,
            spiling_contracts::geometry::MAX_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_canonical(&result);
        assert_eq!(result.loops.as_ptr(), native_loop_buffer);
        assert_eq!(result.loops[0].points_mm.as_ptr(), native_point_buffer);
        result.loops[0].points_mm.reverse();
        assert_eq!(
            canonicalize_section_in_place(
                &mut result,
                spiling_contracts::geometry::MAX_SECTION_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
    }
    assert_eq!(
        source
            .section_with_byte_limit(
                PlaneMm {
                    origin_mm: [0.0, 0.0, 4.0],
                    normal: [0.0, 0.0, 1.0]
                },
                &cancel,
                spiling_contracts::geometry::MAX_SECTION_BYTES,
                &mut GeometryWorkBudget::new(0),
            )
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit,
    );
    for bytes in [0, u32::try_from(definition.id().allocated_bytes()).unwrap()] {
        assert_eq!(
            source
                .section_with_byte_limit(
                    PlaneMm {
                        origin_mm: [0.0, 0.0, 4.0],
                        normal: [0.0, 0.0, 1.0]
                    },
                    &cancel,
                    bytes,
                    &mut work,
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit,
        );
    }
    let cancelled = source
        .section_with_byte_limit(
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
            &AtomicBool::new(true),
            spiling_contracts::geometry::MAX_SECTION_BYTES,
            &mut work,
        )
        .unwrap_err();
    assert_eq!(cancelled.code, GeometryErrorCode::Cancelled);
}

#[test]
fn packed_output_allowance_does_not_fund_native_preparation_or_query_scratch() {
    let definition = fixture("box-mm.step");
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    };
    let cancel = AtomicBool::new(false);
    // A direct call's byte limit owns packed output, not its native seeds.
    let direct = crate::section_with_byte_limit(&definition, plane, &cancel, 960).unwrap();
    let mut work = GeometryWorkBudget::new(2_000_000);
    let source = prepare_section_source(
        &definition,
        MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut work,
    )
    .unwrap();
    let source_bytes = source.retained_bytes();
    // The prepared API instead receives actual remaining shared capacity:
    // enough wire bytes alone must not authorize native scratch growth.
    assert_eq!(
        source
            .section_with_byte_limit(plane, &cancel, 960, &mut work)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
    let available = u32::try_from(MAX_PREPARED_SECTION_BYTES - source_bytes).unwrap();
    let prepared = source
        .section_with_byte_limit(plane, &cancel, available, &mut work)
        .unwrap();
    assert_canonical(&prepared);
    assert_eq!(source.retained_bytes(), source_bytes);
    assert_eq!(prepared.loops.len(), direct.loops.len());
    for (prepared, direct) in prepared.loops.iter().zip(&direct.loops) {
        assert_eq!(prepared.is_hole, direct.is_hole);
        assert_eq!(prepared.points_mm, direct.points_mm);
    }
    assert!((signed_area(&prepared.loops[0], &prepared.frame) - 200.0).abs() < 1e-7);
}

#[test]
fn oblique_sections_preserve_native_box_cylinder_and_hole_area_and_winding() {
    // The small tilt cuts only lateral faces and avoids all original vertices,
    // contained boundary edges, tangencies and source coplanar carriers.
    let normal = [0.05, 0.05, 1.0];
    let area_scale = 1.005_f64.sqrt();
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal,
    };
    for (name, area, loop_count, curved_perimeter) in [
        ("box-mm.step", 200.0, 1, 0.0),
        ("cylinder.step", 25.0 * PI, 1, 10.0 * PI * area_scale),
        (
            "through-hole.step",
            400.0 - 9.0 * PI,
            2,
            6.0 * PI * area_scale,
        ),
        (
            "independent-exporters/occt-cylinder-mm.step",
            25.0 * PI,
            1,
            10.0 * PI * area_scale,
        ),
        (
            "independent-exporters/occt-through-hole-mm.step",
            400.0 - 9.0 * PI,
            2,
            6.0 * PI * area_scale,
        ),
    ] {
        let result = section(&fixture(name), plane, &AtomicBool::new(false)).unwrap();
        assert_eq!(result.loops.len(), loop_count, "{name}");
        assert_canonical(&result);
        let measured: f64 = result
            .loops
            .iter()
            .map(|boundary| signed_area(boundary, &result.frame))
            .sum();
        let allowance = curved_perimeter * SECTION_SAMPLING_TOLERANCE_MM
            + PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2)
            + 1e-6;
        assert!(
            (measured - area * area_scale).abs() <= allowance,
            "{name}: {measured}"
        );
    }
}

#[test]
fn plane_normal_scaling_and_repeated_queries_preserve_canonical_loops() {
    let definition = fixture("box-mm.step");
    let origin_mm = [0.0, 0.0, 4.0];
    let reference = section(
        &definition,
        PlaneMm {
            origin_mm,
            normal: [0.0, 0.0, 1.0],
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    for normal in [[0.0, 0.0, 1e-300], [0.0, 0.0, 1e300], [0.0, 0.0, 1.0]] {
        let result = section(
            &definition,
            PlaneMm { origin_mm, normal },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.plane, reference.plane);
        assert_eq!(result.frame, reference.frame);
        assert_eq!(result.loops.len(), reference.loops.len());
        for (actual, expected) in result.loops.iter().zip(&reference.loops) {
            assert_eq!(actual.is_hole, expected.is_hole);
            assert_eq!(actual.points_mm, expected.points_mm);
        }
    }
    let reverse = section(
        &definition,
        PlaneMm {
            origin_mm,
            normal: [0.0, 0.0, -1.0],
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_canonical(&reverse);
    assert!((signed_area(&reverse.loops[0], &reverse.frame) - 200.0).abs() < 1e-6);
}

#[test]
fn vertex_contained_edge_coplanar_face_and_curved_tangency_are_explicit_failures() {
    let box_definition = fixture("box-mm.step");
    for plane in [
        PlaneMm {
            origin_mm: [0.0; 3],
            normal: [1.0, 1.0, 1.0],
        },
        // Contains the original x-directed edge at y=z=0, without
        // being coplanar with an entire box face.
        PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 1.0, 1.0],
        },
        PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        },
    ] {
        assert_eq!(
            section(&box_definition, plane, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            GeometryErrorCode::DegenerateSection
        );
    }
    let cylinder = fixture("cylinder.step");
    let radial = [0.6, 0.8, 0.0];
    // Interior stationary circle angles, not merely axis-aligned native
    // seam vertices, witness this curved tangent plane.
    for delta in [0.0, SECTION_BOOLEAN_TOLERANCE_MM * 0.5] {
        let plane = PlaneMm {
            origin_mm: radial.map(|value| value * (5.0 + delta)),
            normal: radial,
        };
        assert_eq!(
            section(&cylinder, plane, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            GeometryErrorCode::DegenerateSection
        );
    }
    let outside = section(
        &cylinder,
        PlaneMm {
            origin_mm: radial.map(|value| value * 6.0),
            normal: radial,
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(outside.loops.is_empty());
    // Domain failure never consumes or mutates the borrowed definition.
    assert_eq!(
        section(
            &cylinder,
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
            &AtomicBool::new(false)
        )
        .unwrap()
        .loops
        .len(),
        1
    );
}

#[test]
fn clipping_uses_all_certified_corners_and_exact_unpadded_plane_boundary() {
    let definition = fixture("box-mm.step");
    let frame = PlaneFrameMm::from_plane(PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [1.0, 2.0, 3.0],
    })
    .unwrap();
    let (min, max) = clip_bounds(&definition, &frame).unwrap();
    assert_eq!(min[2].to_bits(), 0.0_f64.to_bits());
    let transform = frame_to_definition(&frame);
    for mask in 0..8 {
        let corner = std::array::from_fn(|axis| {
            if mask & (1 << axis) == 0 {
                definition.bounds.min[axis]
            } else {
                definition.bounds.max[axis]
            }
        });
        let local = in_frame(corner, &frame).unwrap();
        for axis in 0..2 {
            assert!(local[axis] >= min[axis] + 1.0 - 1e-12);
            assert!(local[axis] <= max[axis] - 1.0 + 1e-12);
        }
        assert!(local[2] <= max[2] - 1.0 + 1e-12);
        use monstertruck_modeling::Transform;
        let restored: [f64; 3] = transform
            .transform_point(Point3::new(local[0], local[1], local[2]))
            .into();
        assert!((0..3).all(|axis| (corner[axis] - restored[axis]).abs() < 1e-12));
    }
}

#[test]
fn near_parallel_source_face_is_not_misidentified_as_generated_cap() {
    let definition = fixture("box-mm.step");
    // The retained top face lies inside the cap coincidence tolerance, but
    // outside the degeneracy tolerance. It is still an original carrier.
    let z = 8.0 - SECTION_PLANE_TOLERANCE_MM * 0.5;
    let result = section(
        &definition,
        PlaneMm {
            origin_mm: [0.0, 0.0, z],
            normal: [0.0, 0.0, 1.0],
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.loops.len(), 1);
    assert_canonical(&result);
    assert!(
        result.loops[0]
            .points_mm
            .iter()
            .all(|point| (point[2] - z).abs() < 1e-8)
    );
}

fn assert_analytic_cylinder_public_boolean_preserves_closed_caps(name: &str, source_faces: usize) {
    let definition = fixture(name);
    let source_mapping = definition.faces.clone();
    let source_provenance = definition.provenance.clone();
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    };
    let frame = PlaneFrameMm::from_plane(plane).unwrap();
    let (min, max) = clip_bounds(&definition, &frame).unwrap();
    let clip: Solid = primitive::cuboid(BoundingBox::from_iter([
        Point3::from(min),
        Point3::from(max),
    ]));
    let clip = builder::transformed(&clip, frame_to_definition(&frame));
    let native = monstertruck_solid::and(&definition.solid, &clip, SECTION_BOOLEAN_TOLERANCE_MM)
        .expect("public native AND of the original analytic cylinder and certified clip");
    assert_eq!(native.boundaries().len(), 1);
    native.boundaries()[0].check_solid_boundary().unwrap();
    let result = section(&definition, plane, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.loops.len(), 1);
    assert_canonical(&result);
    for point in &result.loops[0].points_mm {
        assert!((point[0].hypot(point[1]) - 5.0).abs() <= SECTION_PLANE_TOLERANCE_MM);
        assert!((point[2] - 4.0).abs() <= SECTION_PLANE_TOLERANCE_MM);
    }
    let allowance =
        10.0 * PI * SECTION_SAMPLING_TOLERANCE_MM + PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2);
    assert!((signed_area(&result.loops[0], &frame) - 25.0 * PI).abs() <= allowance);
    assert_eq!(definition.solid.face_iter().count(), source_faces);
    assert_eq!(definition.faces, source_mapping);
    assert_eq!(definition.provenance, source_provenance);
}

#[test]
fn cylinder_exporters_public_boolean_preserves_closed_native_caps() {
    assert_analytic_cylinder_public_boolean_preserves_closed_caps("cylinder.step", 4);
    assert_analytic_cylinder_public_boolean_preserves_closed_caps(
        "independent-exporters/occt-cylinder-mm.step",
        3,
    );
}

fn assert_through_hole_public_boolean_preserves_inner_cap_incidence(name: &str) {
    let definition = fixture(name);
    let source_faces = definition.solid.face_iter().count();
    let source_mapping = definition.faces.clone();
    let source_provenance = definition.provenance.clone();
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    };
    let frame = PlaneFrameMm::from_plane(plane).unwrap();
    let (min, max) = clip_bounds(&definition, &frame).unwrap();
    let clip: Solid = primitive::cuboid(BoundingBox::from_iter([
        Point3::from(min),
        Point3::from(max),
    ]));
    let clip = builder::transformed(&clip, frame_to_definition(&frame));
    let native = monstertruck_solid::and(&definition.solid, &clip, SECTION_BOOLEAN_TOLERANCE_MM)
        .expect("public native AND retains both through-hole cap boundaries");
    assert_eq!(native.boundaries().len(), 1);
    native.boundaries()[0].check_solid_boundary().unwrap();
    let result = section(&definition, plane, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.loops.len(), 2);
    assert_canonical(&result);
    assert!(!result.loops[0].is_hole);
    assert!(result.loops[1].is_hole);
    for point in &result.loops[1].points_mm {
        assert!(
            ((point[0] - 10.0).hypot(point[1] - 10.0) - 3.0).abs() <= SECTION_PLANE_TOLERANCE_MM
        );
    }
    let area: f64 = result
        .loops
        .iter()
        .map(|boundary| signed_area(boundary, &frame))
        .sum();
    let allowance =
        6.0 * PI * SECTION_SAMPLING_TOLERANCE_MM + PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2);
    assert!((area - (400.0 - 9.0 * PI)).abs() <= allowance);
    assert_eq!(definition.solid.face_iter().count(), source_faces);
    assert_eq!(definition.faces, source_mapping);
    assert_eq!(definition.provenance, source_provenance);
}

#[test]
fn through_hole_exporters_public_boolean_preserves_inner_cap_edge_incidence() {
    for name in [
        "through-hole.step",
        "independent-exporters/occt-through-hole-mm.step",
    ] {
        assert_through_hole_public_boolean_preserves_inner_cap_incidence(name);
    }
}

#[test]
fn frozen_occt_placed_first_layers_keep_native_caps_and_source_identity() {
    for (name, inverse_pose_xy, area, loop_count, radius, center) in [
        (
            "independent-exporters/occt-cylinder-mm.step",
            [-40.0, -20.0],
            25.0 * PI,
            1,
            5.0,
            [0.0, 0.0],
        ),
        (
            "independent-exporters/occt-through-hole-mm.step",
            [-80.0, 0.0],
            400.0 - 9.0 * PI,
            2,
            3.0,
            [10.0, 10.0],
        ),
    ] {
        let definition = fixture(name);
        let source_mapping = definition.faces.clone();
        let source_provenance = definition.provenance.clone();
        let source_geometry =
            serde_json::to_vec(&definition.solid.boundaries()[0].compress()).unwrap();
        for height in [0.1, 0.3] {
            // Engine occurrence placement inverse-transforms the world plane;
            // the authoritative definition and original STEP bytes stay local.
            let plane = PlaneMm {
                origin_mm: [inverse_pose_xy[0], inverse_pose_xy[1], height],
                normal: [0.0, 0.0, 1.0],
            };
            let frame = PlaneFrameMm::from_plane(plane).unwrap();
            let (min, max) = clip_bounds(&definition, &frame).unwrap();
            let clip: Solid = primitive::cuboid(BoundingBox::from_iter([
                Point3::from(min),
                Point3::from(max),
            ]));
            let clip = builder::transformed(&clip, frame_to_definition(&frame));
            let native =
                monstertruck_solid::and(&definition.solid, &clip, SECTION_BOOLEAN_TOLERANCE_MM)
                    .expect(
                        "original placed first-layer OCCT geometry has a transverse native cap",
                    );
            assert!(native.is_geometric_consistent());
            assert_eq!(native.boundaries().len(), 1);
            native.boundaries()[0].check_solid_boundary().unwrap();
            let result = section(&definition, plane, &AtomicBool::new(false)).unwrap();
            assert_eq!(result.loops.len(), loop_count);
            assert_canonical(&result);
            let circle = &result.loops[loop_count - 1];
            assert_eq!(circle.is_hole, loop_count == 2);
            for point in &circle.points_mm {
                assert!(
                    ((point[0] - center[0]).hypot(point[1] - center[1]) - radius).abs()
                        <= SECTION_PLANE_TOLERANCE_MM
                );
                assert!((point[2] - height).abs() <= SECTION_PLANE_TOLERANCE_MM);
            }
            let measured: f64 = result
                .loops
                .iter()
                .map(|boundary| signed_area(boundary, &frame))
                .sum();
            let allowance = 2.0 * PI * radius * SECTION_SAMPLING_TOLERANCE_MM
                + PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2);
            assert!((measured - area).abs() <= allowance);
            assert_eq!(definition.faces, source_mapping);
            assert_eq!(definition.provenance, source_provenance);
            assert_eq!(
                serde_json::to_vec(&definition.solid.boundaries()[0].compress()).unwrap(),
                source_geometry,
            );
        }
    }
}

#[test]
fn native_projection_mixed_axis_cylinder_is_not_rotated_aabb_contact() {
    let definition = fixture("cylinder.step");
    let faces = definition.faces.clone();
    let provenance = definition.provenance.clone();
    let angle = 0.37f64;
    let direction = [
        angle.sin() / 2.0f64.sqrt(),
        -angle.sin() / 2.0f64.sqrt(),
        angle.cos(),
    ];
    let mut work = crate::GeometryWorkBudget::new(1_000_000);
    let policy = crate::BoundsPolicy {
        maximum_endpoint_uncertainty_mm: 1e-6,
    };
    let extent = crate::projected_bounds(
        &definition,
        direction,
        policy,
        &mut work,
        &AtomicBool::new(false),
    )
    .unwrap();
    // Independent analytic witness: radius5, native height8, center on Z axis.
    let expected = [-5.0 * angle.sin(), 8.0 * angle.cos() + 5.0 * angle.sin()];
    for (interval, expected) in [extent.minimum, extent.maximum].into_iter().zip(expected) {
        assert!(interval[0] <= expected && expected <= interval[1]);
        assert!(interval[1] - interval[0] <= policy.maximum_endpoint_uncertainty_mm);
    }
    let box_min = -5.0 * (direction[0].abs() + direction[1].abs());
    assert!(extent.minimum[0] - box_min > 0.5);
    let first_query_work = work.consumed();
    let scaled = crate::projected_bounds(
        &definition,
        direction.map(|x| x * 1e200),
        policy,
        &mut work,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(work.consumed() > first_query_work);
    for (a, b) in [scaled.minimum, scaled.maximum]
        .into_iter()
        .zip([extent.minimum, extent.maximum])
    {
        assert!((a[0] - b[0]).abs() <= 2.0 * policy.maximum_endpoint_uncertainty_mm);
        assert!((a[1] - b[1]).abs() <= 2.0 * policy.maximum_endpoint_uncertainty_mm);
    }
    assert_eq!(definition.faces, faces);
    assert_eq!(definition.provenance, provenance);
    assert_eq!(
        crate::projected_bounds(
            &definition,
            [0.0; 3],
            policy,
            &mut work,
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::InvalidGeometry
    );
    assert_eq!(
        crate::projected_bounds(
            &definition,
            [f64::NAN, 0.0, 1.0],
            policy,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::InvalidGeometry
    );
    assert_eq!(
        crate::projected_bounds(
            &definition,
            direction,
            policy,
            &mut work,
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::Cancelled
    );
}
