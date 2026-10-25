// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::geometry::{AnalyticSurfaceDescription, GeometryErrorCode};

fn source(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry")
            .join(name),
    )
    .unwrap()
}
fn load(name: &str) -> Definition {
    from_step_bytes_with_cancel(&source(name), &AtomicBool::new(false)).unwrap()
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
        .filter(|face| {
            matches!(
                face.geometry.analytic,
                Some(AnalyticSurfaceDescription::Cylinder { .. })
            )
        })
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
    let definition = from_step_bytes_with_cancel(&source, &AtomicBool::new(false)).unwrap();
    let cancel = AtomicBool::new(true);
    assert_eq!(
        from_step_bytes_with_cancel(&source, &cancel)
            .unwrap_err()
            .code,
        GeometryErrorCode::Cancelled
    );
    assert_eq!(
        tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel)
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
        from_step_bytes_with_limits(&bytes, &cancel, 5, crate::import::MAX_QUERY_PROOF_BYTES)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
    let definition =
        from_step_bytes_with_limits(&bytes, &cancel, 6, crate::import::MAX_QUERY_PROOF_BYTES)
            .unwrap();
    let capacity = MeshBudget {
        vertices: 24,
        triangles: 12,
        bytes: 832,
    };
    let encoded = tessellate_with_budget(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &cancel,
        capacity,
    )
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
            tessellate_with_budget(
                &definition,
                DisplayProfile::MeshTrimmedMm005V2,
                &cancel,
                narrowed
            )
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

#[test]
fn native_box_wall_certificates_keep_exact_face_identity_outward_side_and_source_frame() {
    for name in [
        "box-mm.step",
        "box-inch.step",
        "independent-exporters/occt-box-mm.step",
    ] {
        let definition = load(name);
        let expected = [
            ([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], 10.0),
            ([20.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], 10.0),
            ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0], 20.0),
            ([0.0, 10.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 20.0),
        ];
        let mut seen = [false; 4];
        let mut top_bottom = 0;
        for face in definition.faces() {
            let Some(AnalyticSurfaceDescription::Plane { normal, .. }) = face.geometry.analytic
            else {
                panic!("box carrier");
            };
            if normal[2].abs() > 0.5 {
                assert_eq!(
                    definition
                        .vertical_wall_face(&face.face_id)
                        .unwrap_err()
                        .code,
                    GeometryErrorCode::UnsupportedGeometry
                );
                top_bottom += 1;
                continue;
            }
            let cert = definition.vertical_wall_face(&face.face_id).unwrap();
            let side = expected
                .iter()
                .position(|(_, _, n, _)| {
                    n.iter()
                        .zip(cert.outward_normal)
                        .all(|(a, b)| (a - b).abs() < 1e-10)
                })
                .expect("one of the four independently known outward box sides");
            assert!(
                !seen[side],
                "distinct source face uses cannot share a side certificate"
            );
            seen[side] = true;
            let (origin, horizontal, outward, width) = expected[side];
            for axis in 0..3 {
                close(cert.origin_mm[axis], origin[axis], 1e-7);
                close(cert.horizontal_direction[axis], horizontal[axis], 1e-10);
                close(cert.outward_normal[axis], outward[axis], 1e-10);
            }
            close(cert.width_mm, width, 1e-7);
            close(cert.min_z_mm, 0.0, 1e-7);
            close(cert.max_z_mm, 8.0, 1e-7);
            assert_eq!(cert, definition.vertical_wall_face(&face.face_id).unwrap());
            assert_eq!(
                cert,
                definition
                    .vertical_wall_face_with_cancel(&face.face_id, &AtomicBool::new(false))
                    .unwrap()
            );
        }
        assert_eq!(seen, [true; 4]);
        assert_eq!(top_bottom, 2);
        let unknown = FaceId::from_step_entity(999_999).unwrap();
        assert_eq!(
            definition.vertical_wall_face(&unknown).unwrap_err().code,
            GeometryErrorCode::UnknownHandle
        );
        assert_eq!(
            definition
                .vertical_wall_face_with_cancel(
                    &definition.faces()[0].face_id,
                    &AtomicBool::new(true)
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
    }
}

/// Genuine original planar STEP source, not a mocked certificate or mesh.
fn native_notched_extrusion() -> Definition {
    use monstertruck_io::step::{
        load::step_geometry::{Curve3D, ElementarySurface, Surface, SweepSurface, re_exports::*},
        save::{CompleteStepDisplay, StepHeaderDescriptor, StepModel},
    };
    use monstertruck_modeling::builder;
    use monstertruck_topology::{Face, Wire};
    let polygon = [
        (0.0, 0.0),
        (20.0, 0.0),
        (20.0, 10.0),
        (12.0, 10.0),
        (12.0, 6.0),
        (8.0, 6.0),
        (8.0, 10.0),
        (0.0, 10.0),
    ];
    let vertices = builder::vertices(polygon.into_iter().map(|(x, y)| (x, y, 0.0)));
    let wire: Wire<Point3, Curve3D> = (0..vertices.len())
        .map(|i| builder::line(&vertices[i], &vertices[(i + 1) % vertices.len()]))
        .collect();
    let face: Face<Point3, Curve3D, Surface> = builder::try_attach_plane(vec![wire]).unwrap();
    let solid = builder::extrude(&face, 8.0 * Vector3::unit_z());
    let solid = solid
        .try_mapped(
            |p| Some(*p),
            |curve| Some(curve.clone()),
            |surface| match surface {
                Surface::SweepSurface(SweepSurface::ExtrusionSurface(extrusion)) => {
                    let Curve3D::Line(line) = extrusion.entity_curve().as_ref() else {
                        return None;
                    };
                    Some(Surface::ElementarySurface(ElementarySurface::Plane(
                        Plane::new(line.0, line.1, line.0 + extrusion.extruding_vector()),
                    )))
                }
                surface => Some(surface.clone()),
            },
        )
        .unwrap();
    solid.boundaries()[0].check_solid_boundary().unwrap();
    let compressed = solid.compress();
    let bytes = CompleteStepDisplay::new(
        StepModel::from(&compressed),
        StepHeaderDescriptor {
            file_name: "original-notched-wall-regression.step".into(),
            time_stamp: "2026-01-01T00:00:00".into(),
            authors: vec!["Spiling contributors".into()],
            organization: vec!["Spiling".into()],
            organization_system: "Original native geometry regression".into(),
            authorization: "OSL-3.0".into(),
        },
    )
    .to_string();
    from_step_bytes_with_cancel(bytes.as_bytes(), &AtomicBool::new(false)).unwrap()
}

#[test]
fn recessed_rectangles_and_ambiguous_coplanar_trims_are_not_exterior_wall_certificates() {
    let definition = native_notched_extrusion();
    let recessed = definition.faces().iter().find(|face| matches!(
        face.geometry.analytic,
        Some(AnalyticSurfaceDescription::Plane { origin_mm, normal }) if (origin_mm[1] - 6.0).abs() < 1e-7 && normal[1].abs() > 0.5
    )).expect("genuine full-height four-line notch back wall");
    let error = definition
        .vertical_wall_face(&recessed.face_id)
        .unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::UnsupportedGeometry);
    assert!(error.message.contains("supporting"));
    let coplanar: Vec<_> = definition.faces().iter().filter(|face| matches!(
        face.geometry.analytic,
        Some(AnalyticSurfaceDescription::Plane { origin_mm, normal }) if (origin_mm[1] - 10.0).abs() < 1e-7 && normal[1].abs() > 0.5
    )).collect();
    assert_eq!(coplanar.len(), 2);
    for face in coplanar {
        let error = definition.vertical_wall_face(&face.face_id).unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::UnsupportedGeometry);
        assert!(error.message.contains("ambiguous"));
    }
}

#[test]
fn cylinders_hole_trims_partial_height_and_sloped_native_walls_are_explicitly_unsupported() {
    for name in [
        "cylinder.step",
        "independent-exporters/occt-cylinder-mm.step",
    ] {
        let definition = load(name);
        for face in definition.faces() {
            assert_eq!(
                definition
                    .vertical_wall_face(&face.face_id)
                    .unwrap_err()
                    .code,
                GeometryErrorCode::UnsupportedGeometry
            );
        }
    }
    let hole = load("through-hole.step");
    for face in hole.faces() {
        if matches!(
            face.geometry.analytic,
            Some(AnalyticSurfaceDescription::Cylinder { .. })
        ) || matches!(face.geometry.analytic, Some(AnalyticSurfaceDescription::Plane { normal, .. }) if normal[2].abs() > 0.5)
        {
            assert_eq!(
                hole.vertical_wall_face(&face.face_id).unwrap_err().code,
                GeometryErrorCode::UnsupportedGeometry
            );
        }
    }
    let shelf = load("../manufacturing/native/non-grid-shelf.step");
    let partial = shelf.faces().iter().find(|face| matches!(
        face.geometry.analytic, Some(AnalyticSurfaceDescription::Plane { origin_mm, normal }) if (origin_mm[0] - 20.0).abs() < 1e-7 && normal[0].abs() > 0.5
    )).expect("short exterior vertical shelf wall");
    let error = shelf.vertical_wall_face(&partial.face_id).unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::UnsupportedGeometry);
    assert!(error.message.contains("complete local source height"));
    let ramp = load("../manufacturing/native/inward-planar-ramp.step");
    let sloped = ramp.faces().iter().find(|face| matches!(
        face.geometry.analytic, Some(AnalyticSurfaceDescription::Plane { normal, .. }) if normal[0].abs() > 0.1 && normal[2].abs() > 0.1
    )).expect("genuine sloped native wall");
    assert_eq!(
        ramp.vertical_wall_face(&sloped.face_id).unwrap_err().code,
        GeometryErrorCode::UnsupportedGeometry
    );
}

#[test]
fn wall_certificate_rejects_inconsistent_source_orientation_and_carrier() {
    let definition = load("box-mm.step");
    let index = definition
        .faces()
        .iter()
        .position(|face| {
            matches!(
                face.geometry.analytic, Some(AnalyticSurfaceDescription::Plane { normal, .. }) if normal[2].abs() < 0.5
            )
        })
        .unwrap();
    let face_id = definition.faces[index].face_id.clone();
    let mut definition = load("box-mm.step");
    definition.faces[index].orientation = !definition.faces[index].orientation;
    assert_eq!(
        definition.vertical_wall_face(&face_id).unwrap_err().code,
        GeometryErrorCode::KernelFailure
    );
    if let Some(AnalyticSurfaceDescription::Plane { normal, .. }) =
        &mut definition.faces[index].geometry.analytic
    {
        normal[0] = f64::NAN;
    }
    assert_eq!(
        definition.vertical_wall_face(&face_id).unwrap_err().code,
        GeometryErrorCode::KernelFailure
    );
}

#[test]
fn occt_full_cylinder_certificate_preserves_source_frame_and_refusals() {
    let definition = load("independent-exporters/occt-cylinder-mm.step");
    let side = definition
        .faces()
        .iter()
        .find(|face| {
            matches!(
                face.geometry.analytic,
                Some(AnalyticSurfaceDescription::Cylinder { .. })
            )
        })
        .unwrap();
    let cert = definition.vertical_cylinder_face(&side.face_id).unwrap();
    assert_eq!(cert.center_mm, [0.0, 0.0]);
    close(cert.radius_mm, 5.0, 1e-7);
    close(cert.min_z_mm, 0.0, 1e-7);
    close(cert.max_z_mm, 8.0, 1e-7);
    assert_eq!(
        cert,
        definition.vertical_cylinder_face(&side.face_id).unwrap()
    );
    assert_eq!(
        cert,
        definition
            .vertical_cylinder_face_with_cancel(&side.face_id, &AtomicBool::new(false))
            .unwrap()
    );
    assert_eq!(
        definition
            .vertical_cylinder_face_with_cancel(&side.face_id, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        GeometryErrorCode::Cancelled
    );
    assert_eq!(
        definition
            .vertical_cylinder_face(&FaceId::from_step_entity(999_999).unwrap())
            .unwrap_err()
            .code,
        GeometryErrorCode::UnknownHandle
    );
}

#[test]
fn split_cylinders_bores_and_planar_faces_do_not_acquire_full_cylinder_certificates() {
    for name in [
        "cylinder.step",
        "through-hole.step",
        "independent-exporters/occt-through-hole-mm.step",
        "box-mm.step",
    ] {
        let definition = load(name);
        for face in definition.faces() {
            assert_eq!(
                definition
                    .vertical_cylinder_face(&face.face_id)
                    .unwrap_err()
                    .code,
                GeometryErrorCode::UnsupportedGeometry,
                "{name} {:?}",
                face.face_id
            );
        }
    }
}

/// Export genuine transformed analytic STEP, then exercise normal byte admission.
/// No certificate or modeled NURBS fitting is involved in constructing this input.
fn transformed_periodic_cylinder_source(
    matrix: monstertruck_modeling::base::Matrix4,
) -> Definition {
    use monstertruck_io::step::{
        load::Table,
        save::{CompleteStepDisplay, StepHeaderDescriptor, StepModel},
    };
    let bytes = source("independent-exporters/occt-cylinder-mm.step");
    let exchange = step_p21::parser::parse(std::str::from_utf8(&bytes).unwrap()).unwrap();
    let table = Table::from_data_section(&exchange.data[0]);
    let (compressed, reports) = table
        .to_compressed_solid_reported(table.manifold_solid_brep.values().next().unwrap())
        .unwrap();
    assert!(reports.iter().all(|r| r.is_lossless()));
    let native = monstertruck_topology::Solid::extract(compressed).unwrap();
    let transformed = monstertruck_modeling::builder::transformed(&native, matrix);
    let bytes = CompleteStepDisplay::new(
        StepModel::from(&transformed.compress()),
        StepHeaderDescriptor {
            file_name: "transformed-periodic-cylinder-regression.step".into(),
            time_stamp: "2026-01-01T00:00:00".into(),
            authors: vec!["Spiling contributors".into()],
            organization: vec!["Spiling".into()],
            organization_system: "Native analytic transformation regression".into(),
            authorization: "OSL-3.0".into(),
        },
    )
    .to_string();
    from_step_bytes_with_cancel(bytes.as_bytes(), &AtomicBool::new(false)).unwrap()
}

#[test]
fn cylinder_certificate_is_independent_of_translation_phase_radius_and_axis_sign() {
    use monstertruck_modeling::base::*;
    for rotation in [
        Matrix4::from_angle_z(Rad(0.713)),
        Matrix4::from_angle_x(Rad(std::f64::consts::PI)),
    ] {
        let matrix = Matrix4::from_translation(Vector3::new(13.0, -7.0, 2.0))
            * rotation
            * Matrix4::from_scale(0.5);
        let definition = transformed_periodic_cylinder_source(matrix);
        let side = definition
            .faces()
            .iter()
            .find(|f| {
                matches!(
                    f.geometry.analytic,
                    Some(AnalyticSurfaceDescription::Cylinder { .. })
                )
            })
            .unwrap();
        let cert = definition.vertical_cylinder_face(&side.face_id).unwrap();
        close(cert.center_mm[0], 13.0, 1e-7);
        close(cert.center_mm[1], -7.0, 1e-7);
        close(cert.radius_mm, 2.5, 1e-7);
        let expected = if rotation == Matrix4::from_angle_z(Rad(0.713)) {
            [2.0, 6.0]
        } else {
            [-2.0, 2.0]
        };
        close(cert.min_z_mm, expected[0], 1e-7);
        close(cert.max_z_mm, expected[1], 1e-7);
    }
    let sloped = transformed_periodic_cylinder_source(Matrix4::from_angle_y(Rad(0.2)));
    let side = sloped
        .faces()
        .iter()
        .find(|f| {
            matches!(
                f.geometry.analytic,
                Some(AnalyticSurfaceDescription::Cylinder { .. })
            )
        })
        .unwrap();
    assert_eq!(
        sloped
            .vertical_cylinder_face(&side.face_id)
            .unwrap_err()
            .code,
        GeometryErrorCode::UnsupportedGeometry
    );
}

/// Genuine original analytic source: one periodic cylindrical face per step,
/// with shared native seams and arbitrary exact circular rim subdivisions.
fn original_periodic_cylinder_source(subdivisions: usize, stepped: bool) -> Definition {
    use monstertruck_io::step::{
        load::step_geometry::{Curve3D, ElementarySurface, Surface, re_exports::*},
        save::{CompleteStepDisplay, StepHeaderDescriptor, StepModel},
    };
    use monstertruck_modeling::builder;
    use monstertruck_topology::{Face, Shell, Solid, Wire};
    type StepWire = Wire<Point3, Curve3D>;
    type StepFace = Face<Point3, Curve3D, Surface>;
    let circle = |radius: f64, z: f64| -> StepWire {
        builder::revolve(
            &builder::vertex((radius, 0.0, z)),
            Point3::new(0.0, 0.0, z),
            Vector3::unit_z(),
            builder::SweepAngle::Closed,
            subdivisions,
        )
    };
    let side =
        |bottom: &StepWire, top: &StepWire, radius: f64, min_z: f64, max_z: f64| -> StepFace {
            let seam = builder::line::<Curve3D>(bottom[0].front(), top[0].front());
            let wire = bottom
                .iter()
                .cloned()
                .chain(std::iter::once(seam.clone()))
                .chain(top.inverse())
                .chain(std::iter::once(seam.inverse()))
                .collect();
            let mut carrier = Processor::new(RevolutionSurface::by_revolution(
                Line(
                    Point3::new(radius, 0.0, min_z),
                    Point3::new(radius, 0.0, max_z),
                ),
                Point3::new(0.0, 0.0, min_z),
                Vector3::unit_z(),
            ));
            let point = carrier.subs(0.37, 0.73);
            if carrier
                .normal(0.37, 0.73)
                .dot(Vector3::new(point.x, point.y, 0.0))
                < 0.0
            {
                carrier.invert();
            }
            Face::try_new(
                vec![wire],
                Surface::ElementarySurface(ElementarySurface::CylindricalSurface(carrier)),
            )
            .unwrap()
        };
    let bottom = circle(5.0, 0.0);
    let shoulder_outer = circle(5.0, 8.0);
    let bottom_cap: StepFace = builder::try_attach_plane(vec![bottom.clone()]).unwrap();
    let mut faces = vec![
        bottom_cap.inverse(),
        side(&bottom, &shoulder_outer, 5.0, 0.0, 8.0),
    ];
    if stepped {
        let shoulder_inner = circle(3.0, 8.0);
        let top = circle(3.0, 12.0);
        faces.push(
            builder::try_attach_plane(vec![shoulder_outer, shoulder_inner.inverse()]).unwrap(),
        );
        faces.push(side(&shoulder_inner, &top, 3.0, 8.0, 12.0));
        faces.push(builder::try_attach_plane(vec![top]).unwrap());
    } else {
        faces.push(builder::try_attach_plane(vec![shoulder_outer]).unwrap());
    }
    let shell: Shell<_, _, _> = faces.into_iter().collect();
    let solid = Solid::try_new(vec![shell]).unwrap();
    let bytes = CompleteStepDisplay::new(
        StepModel::from(&solid.compress()),
        StepHeaderDescriptor {
            file_name: "original-periodic-cylinder-regression.step".into(),
            time_stamp: "2026-01-01T00:00:00".into(),
            authors: vec!["Spiling contributors".into()],
            organization: vec!["Spiling".into()],
            organization_system: "Original analytic periodic topology regression".into(),
            authorization: "OSL-3.0".into(),
        },
    )
    .to_string();
    from_step_bytes_with_cancel(bytes.as_bytes(), &AtomicBool::new(false)).unwrap()
}

#[test]
fn arbitrary_native_rim_subdivisions_preserve_full_face_but_short_steps_refuse() {
    for subdivisions in [3, 7] {
        let definition = original_periodic_cylinder_source(subdivisions, false);
        let side = definition
            .faces()
            .iter()
            .find(|f| {
                matches!(
                    f.geometry.analytic,
                    Some(AnalyticSurfaceDescription::Cylinder { .. })
                )
            })
            .unwrap();
        let cert = definition.vertical_cylinder_face(&side.face_id).unwrap();
        close(cert.radius_mm, 5.0, 1e-7);
        close(cert.min_z_mm, 0.0, 1e-7);
        close(cert.max_z_mm, 8.0, 1e-7);
        let stepped = original_periodic_cylinder_source(subdivisions, true);
        for side in stepped.faces().iter().filter(|f| {
            matches!(
                f.geometry.analytic,
                Some(AnalyticSurfaceDescription::Cylinder { .. })
            )
        }) {
            let error = stepped.vertical_cylinder_face(&side.face_id).unwrap_err();
            assert_eq!(error.code, GeometryErrorCode::UnsupportedGeometry);
        }
    }
}
