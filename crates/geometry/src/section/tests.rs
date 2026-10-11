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

#[test]
fn original_analytic_cylinder_public_boolean_preserves_closed_native_caps() {
    // Source: examples/generate_corpus.rs, extruded(circle(0, 0, 5), 8).
    // This is the original analytic cylinder, not a NURBS surface substitute.
    // The imported source and its face slots stay intact.
    let definition = fixture("cylinder.step");
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
    assert_eq!(definition.solid.face_iter().count(), 4);
}

#[test]
fn original_through_hole_public_boolean_preserves_inner_cap_edge_incidence() {
    // Source: examples/generate_corpus.rs, the 20x20x8 original extrusion
    // with the inverted radius-3 circular boundary centered at (10, 10).
    let definition = fixture("through-hole.step");
    let source_faces = definition.solid.face_iter().count();
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
}
