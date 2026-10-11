// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::geometry::{FaceCarrier, GeometryErrorCode};

fn source(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry")
            .join(name),
    )
    .unwrap()
}
fn load(name: &str) -> Definition {
    import_step(&source(name), &AtomicBool::new(false)).unwrap()
}
fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} differs from {expected} by more than {tolerance}"
    );
}
fn area(result: &NativeSection, boundary: &SectionLoopMm) -> f64 {
    let xy = |point: [f64; 3]| {
        let delta: [f64; 3] = std::array::from_fn(|i| point[i] - result.frame.origin_mm[i]);
        let dot = |axis: [f64; 3]| delta.iter().zip(axis).map(|(x, y)| x * y).sum::<f64>();
        [dot(result.frame.x_axis), dot(result.frame.y_axis)]
    };
    boundary
        .points_mm
        .windows(2)
        .map(|pair| {
            let a = xy(pair[0]);
            let b = xy(pair[1]);
            a[0] * b[1] - a[1] * b[0]
        })
        .sum::<f64>()
        * 0.5
}
fn midplane(definition: &Definition) -> NativeSection {
    let bounds = definition.bounds_mm();
    section(
        definition,
        PlaneMm {
            origin_mm: [0.0, 0.0, (bounds.min[2] + bounds.max[2]) * 0.5],
            normal: [0.0, 0.0, 1.0],
        },
        &AtomicBool::new(false),
    )
    .unwrap()
}

#[test]
fn box_exporters_and_units_preserve_geometry_without_erasing_source_identity() {
    let millimetre = load("box-mm.step");
    let inch = load("box-inch.step");
    let occt = load("independent-exporters/occt-box-mm.step");
    for definition in [&millimetre, &inch, &occt] {
        assert_eq!(definition.faces().len(), 6);
        for axis in 0..3 {
            close(definition.bounds_mm().min[axis], 0.0, 1e-6);
            close(
                definition.bounds_mm().max[axis],
                [20.0, 10.0, 8.0][axis],
                1e-6,
            );
        }
        let cut = midplane(definition);
        assert_eq!(cut.loops.len(), 1);
        assert!(!cut.loops[0].is_hole);
        close(area(&cut, &cut.loops[0]), 200.0, 1e-6);
    }
    assert_ne!(millimetre.id(), inch.id());
    assert_ne!(millimetre.id(), occt.id());
}

#[test]
fn native_curved_sections_preserve_material_and_hole_winding() {
    let cylinder = load("cylinder.step");
    let cylinder_section = midplane(&cylinder);
    assert_eq!(cylinder_section.loops.len(), 1);
    close(
        area(&cylinder_section, &cylinder_section.loops[0]),
        25.0 * std::f64::consts::PI,
        10.0 * std::f64::consts::PI * 0.005 + std::f64::consts::PI * 0.005_f64.powi(2),
    );
    let hole = load("through-hole.step");
    let cut = midplane(&hole);
    assert_eq!(cut.loops.len(), 2);
    assert!(!cut.loops[0].is_hole);
    assert!(cut.loops[1].is_hole);
    close(area(&cut, &cut.loops[0]), 400.0, 1e-6);
    close(
        area(&cut, &cut.loops[1]),
        -9.0 * std::f64::consts::PI,
        6.0 * std::f64::consts::PI * 0.005 + std::f64::consts::PI * 0.005_f64.powi(2),
    );
    let cylindrical: Vec<_> = hole
        .faces()
        .iter()
        .filter(|face| matches!(face.carrier, FaceCarrier::Cylinder { .. }))
        .collect();
    assert!(!cylindrical.is_empty());
    for face in cylindrical {
        assert!(
            !inspect_face(&hole, &face.face_id).unwrap().orientation,
            "hole wall must face toward the hole axis"
        );
    }
}

#[test]
fn cancelled_native_operations_never_publish_partial_geometry() {
    let source = source("box-mm.step");
    let definition = import_step(&source, &AtomicBool::new(false)).unwrap();
    let cancel = AtomicBool::new(true);
    assert_eq!(
        import_step(&source, &cancel).unwrap_err().code,
        GeometryErrorCode::Cancelled
    );
    assert_eq!(
        tessellate(&definition, DisplayProfile::MeshMm005V1, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::Cancelled
    );
    assert_eq!(
        section(
            &definition,
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0]
            },
            &cancel
        )
        .unwrap_err()
        .code,
        GeometryErrorCode::Cancelled
    );
}

#[test]
fn outside_and_degenerate_sections_have_distinct_results() {
    let definition = load("box-mm.step");
    let cancel = AtomicBool::new(false);
    let outside = section(
        &definition,
        PlaneMm {
            origin_mm: [0.0, 0.0, 9.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    )
    .unwrap();
    assert!(outside.loops.is_empty());
    let boundary = section(
        &definition,
        PlaneMm {
            origin_mm: [0.0, 0.0, 8.0],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
    )
    .unwrap_err();
    assert_eq!(boundary.code, GeometryErrorCode::DegenerateSection);
}

#[test]
fn remaining_scene_capacity_rejects_without_partial_native_artifacts() {
    let bytes = source("box-mm.step");
    let cancel = AtomicBool::new(false);
    assert_eq!(
        import_step_with_face_limit(&bytes, &cancel, 5)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
    let definition = import_step_with_face_limit(&bytes, &cancel, 6).unwrap();
    let capacity = MeshBudget {
        vertices: 24,
        triangles: 12,
        bytes: 832,
    };
    let encoded =
        tessellate_with_budget(&definition, DisplayProfile::MeshMm005V1, &cancel, capacity)
            .unwrap();
    assert_eq!(
        (
            encoded.vertex_count,
            encoded.triangle_count,
            encoded.total_bytes
        ),
        (24, 12, 832)
    );
    for narrowed in [
        MeshBudget {
            vertices: 23,
            ..capacity
        },
        MeshBudget {
            triangles: 11,
            ..capacity
        },
        MeshBudget {
            bytes: 831,
            ..capacity
        },
    ] {
        assert_eq!(
            tessellate_with_budget(&definition, DisplayProfile::MeshMm005V1, &cancel, narrowed)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
    }
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    };
    assert_eq!(
        section_with_byte_limit(&definition, plane, &cancel, 959)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
    let cut = section_with_byte_limit(&definition, plane, &cancel, 960).unwrap();
    close(area(&cut, &cut.loops[0]), 200.0, 1e-6);
}
