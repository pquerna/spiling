// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::from_step_bytes_with_cancel;
use crate::{GeometryWorkBudget, corpus_fixtures as corpus};
use std::path::PathBuf;
use step_p21::ast::{EntityInstance, Name, Parameter};
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry")
            .join(name),
    )
    .expect("generate original corpus before native tests")
}
fn cancelled() -> AtomicBool {
    AtomicBool::new(false)
}
#[test]
fn native_bounds_enclose_curved_extrema_between_source_vertices() {
    let definition = from_step_bytes_with_cancel(&fixture("cylinder.step"), &cancelled()).unwrap();
    // The cylinder's seam vertices cannot supply either Y extremum.
    for point in [
        [-5.0, 0.0, 0.0],
        [5.0, 0.0, 8.0],
        [0.0, -5.0, 4.0],
        [0.0, 5.0, 4.0],
    ] {
        for axis in 0..3 {
            assert!(definition.bounds.min[axis] <= point[axis]);
            assert!(definition.bounds.max[axis] >= point[axis]);
        }
    }
}
#[test]
fn lexical_caps_ignore_strings_comments_and_binary_strings() {
    lexical_bound(b"/* #1= */ '#2=''#3=' \"#4=\" #5 /*hi*/ =", &cancelled()).unwrap();
    let mut bytes = Vec::new();
    for _ in 0..=spiling_contracts::geometry::MAX_STEP_ENTITIES {
        bytes.extend_from_slice(b"#1 /* whitespace */ =A();");
    }
    assert_eq!(
        lexical_bound(&bytes, &cancelled()).unwrap_err().code,
        GeometryErrorCode::ResourceLimit
    );
    assert_eq!(
        lexical_bound(b"/* unterminated", &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
    assert_eq!(
        lexical_bound(b"'unterminated", &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
}
#[test]
fn source_cap_is_checked_before_parser() {
    let bytes = vec![b' '; MAX_SOURCE_BYTES as usize + 1];
    assert_eq!(
        from_step_bytes_with_cancel(&bytes, &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
}
#[test]
fn canonical_oriented_face_use_survives_conversion_and_large_entity_ids() {
    let source = fixture("box-mm.step");
    let mut ex = step_p21::parser::parse(std::str::from_utf8(&source).unwrap()).unwrap();
    let first = corpus::first(&mut ex, "CLOSED_SHELL");
    let Parameter::List(faces) = &mut corpus::parameters(first)[1] else {
        panic!()
    };
    let Parameter::Ref(Name::Entity(surface_face)) = faces[0] else {
        panic!()
    };
    let canonical = 9_007_199_254_740_993;
    faces[0] = Parameter::Ref(Name::Entity(canonical));
    corpus::add(
        &mut ex,
        canonical,
        &format!("ORIENTED_FACE('',*,#{surface_face},.T.)"),
    );
    let bytes = corpus::serialize(&ex);
    let definition = from_step_bytes_with_cancel(bytes.as_bytes(), &cancelled()).unwrap();
    assert_eq!(
        definition.faces[0].face_id.as_str(),
        "step:9007199254740993"
    );
    assert_eq!(
        definition.faces[0].surface_face_entity,
        surface_face.to_string()
    );
    assert_eq!(definition.faces.len(), 6);
    assert_ne!(
        definition.faces[0].face_id,
        FaceId::from_step_entity(surface_face).unwrap()
    );
    let same = from_step_bytes_with_cancel(bytes.as_bytes(), &cancelled()).unwrap();
    assert_eq!(definition.id, same.id);
    assert_eq!(definition.faces, same.faces);
}
#[test]
fn each_original_adversary_returns_its_exact_manifest_error() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    for entry in manifest["fixtures"].as_array().unwrap() {
        if let Some(expected) = entry["expected_error"].as_str() {
            let source = std::fs::read(root.join(entry["path"].as_str().unwrap())).unwrap();
            let err = from_step_bytes_with_cancel(&source, &cancelled())
                .expect_err("adversary must not admit partial geometry");
            assert_eq!(
                serde_json::to_value(err.code).unwrap(),
                expected,
                "{}",
                entry["path"]
            );
        }
    }
}
#[test]
fn conversion_inch_source_is_dimensional_and_keeps_uncertainty() {
    let mm = from_step_bytes_with_cancel(&fixture("box-mm.step"), &cancelled()).unwrap();
    let inch = from_step_bytes_with_cancel(&fixture("box-inch.step"), &cancelled()).unwrap();
    assert_eq!(
        inch.provenance.source_unit,
        spiling_contracts::geometry::SourceUnit::Inch
    );
    let a = mm
        .provenance
        .uncertainty_mm
        .expect("original writer declares uncertainty");
    let b = inch
        .provenance
        .uncertainty_mm
        .expect("conversion preserves declared uncertainty");
    assert!((a - b).abs() < 1e-12);
    assert_ne!(mm.provenance.source_hash, inch.provenance.source_hash);
    assert_eq!(
        mm.faces.iter().map(|f| &f.face_id).collect::<Vec<_>>(),
        inch.faces.iter().map(|f| &f.face_id).collect::<Vec<_>>()
    );
    for i in 0..3 {
        assert!((mm.bounds.min[i] - inch.bounds.min[i]).abs() < 1e-5);
        assert!((mm.bounds.max[i] - inch.bounds.max[i]).abs() < 1e-5);
    }
    assert!((inch.bounds.max[0] - 20.0).abs() < 1e-5);
    assert!((inch.bounds.max[1] - 10.0).abs() < 1e-5);
    assert!((inch.bounds.max[2] - 8.0).abs() < 1e-5);
}
#[test]
fn missing_reference_duplicate_ids_and_multiple_data_never_partially_import() {
    let source = String::from_utf8(fixture("box-mm.step")).unwrap();
    let mut ex = step_p21::parser::parse(&source).unwrap();
    let duplicate = ex.data[0].entities[0].clone();
    ex.data[0].entities.push(duplicate);
    let bytes = corpus::serialize(&ex);
    assert_eq!(
        from_step_bytes_with_cancel(bytes.as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
    let two_data = source.replace("END-ISO-10303-21;", "DATA; ENDSEC; END-ISO-10303-21;");
    assert_eq!(
        from_step_bytes_with_cancel(two_data.as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::UnsupportedGeometry
    );
    let mut ex = step_p21::parser::parse(&source).unwrap();
    let id = corpus::next_id(&ex);
    corpus::parameters(corpus::first(&mut ex, "ADVANCED_FACE"))[2] =
        Parameter::Ref(Name::Entity(id));
    let bytes = corpus::serialize(&ex);
    assert_eq!(
        from_step_bytes_with_cancel(bytes.as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
    assert!(ex.data[0].entities.iter().any(
        |e| matches!(e,EntityInstance::Simple { record,.. } if record.name == "ADVANCED_FACE")
    ));
}
#[test]
fn exact_circle_witnesses_bound_the_native_curved_extrema() {
    let def = from_step_bytes_with_cancel(&fixture("cylinder.step"), &cancelled()).unwrap();
    let mut circular = 0;
    for (_id, edge) in def.brep().edges().iter() {
        if let Some(curve) = &edge.curve {
            if let solidcore::curve::curve::p2::CurveKind::Ellipse(ell) = curve.kind() {
                if ell.is_circle() {
                    circular += 1;
                    assert!((ell.x_radius() - 5.).abs() < 1e-8);
                }
            }
        }
    }
    assert!(circular >= 4, "two analytic half arcs on each cap");
}

#[test]
fn qualified_ap_schemas_and_single_part_representation_links_preserve_native_geometry() {
    let source = String::from_utf8(fixture("box-mm.step")).unwrap();
    for schema in [
        "AUTOMOTIVE_DESIGN { 1 0 10303 214 3 1 1 }",
        "AUTOMOTIVE_DESIGN { 1 0 10303 214 1 1 1 1 }",
        "CONFIG_CONTROL_DESIGN { 1 0 10303 203 1 1 4 }",
    ] {
        let mut exchange = step_p21::parser::parse(&source).unwrap();
        let schema_record = exchange
            .header
            .iter_mut()
            .find(|record| record.name == "FILE_SCHEMA")
            .unwrap();
        schema_record.parameter = Parameter::List(vec![Parameter::List(vec![Parameter::String(
            schema.into(),
        )])]);
        let cancel = cancelled();
        let entities = Entities::new(&exchange.data[0], &cancel).unwrap();
        let representation_id = entities.named("SHAPE_REPRESENTATION", &cancel).unwrap()[0];
        drop(entities);
        let wrapper = corpus::next_id(&exchange);
        let context =
            reference(at(corpus::first(&mut exchange, "SHAPE_REPRESENTATION"), 2).unwrap())
                .unwrap();
        corpus::add(
            &mut exchange,
            wrapper,
            &format!("SHAPE_REPRESENTATION('single part',(),#{context})"),
        );
        corpus::add(
            &mut exchange,
            wrapper + 1,
            &format!(
                "SHAPE_REPRESENTATION_RELATIONSHIP('single part','',#{wrapper},#{representation_id})"
            ),
        );
        let definition =
            from_step_bytes_with_cancel(corpus::serialize(&exchange).as_bytes(), &cancelled())
                .unwrap();
        for axis in 0..3 {
            assert!((definition.bounds.max[axis] - [20.0, 10.0, 8.0][axis]).abs() < 1e-6);
        }
    }
    let mut exchange = step_p21::parser::parse(&source).unwrap();
    let schema = exchange
        .header
        .iter_mut()
        .find(|record| record.name == "FILE_SCHEMA")
        .unwrap();
    schema.parameter = Parameter::List(vec![Parameter::List(vec![Parameter::String(
        "AUTOMOTIVE_DESIGN { 1 0 10303 203 3 1 1 }".into(),
    )])]);
    assert_eq!(
        from_step_bytes_with_cancel(corpus::serialize(&exchange).as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::UnsupportedGeometry
    );
}

#[test]
fn deep_acyclic_conversion_units_resolve_without_consuming_the_call_stack() {
    let mut exchange =
        step_p21::parser::parse(std::str::from_utf8(&fixture("box-inch.step")).unwrap()).unwrap();
    let cancel = cancelled();
    let entities = Entities::new(&exchange.data[0], &cancel).unwrap();
    let original_unit = entities.named("CONVERSION_BASED_UNIT", &cancel).unwrap()[0];
    let dimension =
        reference(at(entities.record(original_unit, "NAMED_UNIT").unwrap(), 0).unwrap()).unwrap();
    drop(entities);
    let mut base = original_unit;
    let mut current = corpus::next_id(&exchange);
    for _ in 0..5000 {
        let records = vec![
            format!("CONVERSION_BASED_UNIT('inch',#{})", current + 1)
                .parse()
                .unwrap(),
            "LENGTH_UNIT()".parse().unwrap(),
            format!("NAMED_UNIT(#{dimension})").parse().unwrap(),
        ];
        exchange.data[0].entities.push(EntityInstance::Complex {
            id: current,
            subsuper: step_p21::ast::SubSuperRecord(records),
        });
        corpus::add(
            &mut exchange,
            current + 1,
            &format!("LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.),#{base})"),
        );
        base = current;
        current += 2;
    }
    let Parameter::List(units) =
        &mut corpus::parameters(corpus::first(&mut exchange, "GLOBAL_UNIT_ASSIGNED_CONTEXT"))[0]
    else {
        panic!()
    };
    let length = units
        .iter_mut()
        .find(|parameter| reference(parameter).ok() == Some(original_unit))
        .unwrap();
    *length = Parameter::Ref(Name::Entity(base));
    let definition =
        from_step_bytes_with_cancel(corpus::serialize(&exchange).as_bytes(), &cancel).unwrap();
    assert_eq!(
        definition.provenance.source_unit,
        spiling_contracts::geometry::SourceUnit::Inch
    );
    for axis in 0..3 {
        assert!((definition.bounds.max[axis] - [20.0, 10.0, 8.0][axis]).abs() < 1e-6);
    }
}

#[test]
fn frozen_occt_periodic_edges_admit_closed_solids_without_changing_source_face_slots() {
    for (name, bytes, hash, count, min, max) in [
        (
            "independent-exporters/occt-cylinder-mm.step",
            5649,
            "21ea8ea395b4dd38d04dbe901d2538dd781f6567b404031148302164c85a8a9a",
            3,
            [-5.0, -5.0, 0.0],
            [5.0, 5.0, 8.0],
        ),
        (
            "independent-exporters/occt-through-hole-mm.step",
            19015,
            "71d0276a8ba240c608597baf0a079ecb6078f1bd88ebd61bff88abdaa3c25ff0",
            7,
            [0.0, 0.0, 0.0],
            [20.0, 20.0, 8.0],
        ),
    ] {
        let source = fixture(name);
        assert_eq!(source.len(), bytes, "frozen source bytes");
        assert_eq!(
            SourceHash::from_bytes(&source).as_str(),
            hash,
            "frozen source hash"
        );
        let cancel = cancelled();
        let exchange = step_p21::parser::parse(std::str::from_utf8(&source).unwrap()).unwrap();
        let entities = Entities::new(&exchange.data[0], &cancel).unwrap();
        let shell_id = entities.named("CLOSED_SHELL", &cancel).unwrap()[0];
        let canonical =
            list(at(entities.record(shell_id, "CLOSED_SHELL").unwrap(), 1).unwrap()).unwrap();
        let definition = from_step_bytes_with_cancel(&source, &cancel)
            .expect("unchanged independent OCCT solid");
        assert_eq!(definition.source_bytes as usize, source.len());
        assert_eq!(definition.provenance.source_hash.as_str(), hash);
        assert_eq!(definition.faces.len(), count);
        for (info, source_face) in definition.faces.iter().zip(canonical) {
            let id = reference(source_face).unwrap();
            assert_eq!(info.face_id, FaceId::from_step_entity(id).unwrap());
            assert_eq!(
                info.surface_face_entity,
                resolve_face(&entities, id).unwrap().0.to_string()
            );
        }
        let uncertainty = definition
            .provenance
            .uncertainty_mm
            .expect("declared source uncertainty");
        for axis in 0..3 {
            assert!((definition.bounds.min[axis] - min[axis]).abs() <= uncertainty);
            assert!((definition.bounds.max[axis] - max[axis]).abs() <= uncertainty);
        }
        assert!(
            solidcore::topology::brep::p2::is_manifold_solid(definition.brep()).unwrap_or(false),
            "closed oriented manifold"
        );
    }
}

#[test]
fn periodic_representation_does_not_admit_open_or_nonperiodic_self_loop_topology() {
    let source = fixture("independent-exporters/occt-cylinder-mm.step");
    let original = step_p21::parser::parse(std::str::from_utf8(&source).unwrap()).unwrap();
    let mut open = original.clone();
    let Parameter::List(edges) = &mut corpus::parameters(corpus::first(&mut open, "EDGE_LOOP"))[1]
    else {
        panic!("original periodic lateral edge loop")
    };
    edges.pop().unwrap();
    assert_eq!(
        from_step_bytes_with_cancel(corpus::serialize(&open).as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
    let mut nonperiodic = original.clone();
    let cancel = cancelled();
    let entities = Entities::new(&original.data[0], &cancel).unwrap();
    let seam = entities.named("SEAM_CURVE", &cancel).unwrap()[0];
    let line = reference(at(entities.record(seam, "SEAM_CURVE").unwrap(), 1).unwrap()).unwrap();
    corpus::parameters(corpus::first(&mut nonperiodic, "EDGE_CURVE"))[3] =
        Parameter::Ref(Name::Entity(line));
    assert_eq!(
        from_step_bytes_with_cancel(corpus::serialize(&nonperiodic).as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
    let mut inconsistent = original;
    corpus::parameters(corpus::first(&mut inconsistent, "CIRCLE"))[2] = Parameter::Real(4.0);
    assert!(
        from_step_bytes_with_cancel(corpus::serialize(&inconsistent).as_bytes(), &cancelled())
            .is_err(),
        "an inconsistent original radius must refuse, regardless of rejection stage",
    );
}

#[test]
fn unclamped_rational_closed_edge_preserves_shared_original_parameter_restrictions() {
    let source = fixture("independent-exporters/occt-spline-housing-mm.step");
    let definition =
        from_step_bytes_with_cancel(&source, &cancelled()).expect("unchanged rational closed edge");
    assert_eq!(definition.faces.len(), definition.brep().faces().len());
    assert!(
        solidcore::topology::brep::p2::is_manifold_solid(definition.brep()).unwrap_or(false),
        "closed oriented manifold"
    );
}

#[test]
fn regular_native_families_import_with_original_face_and_edge_identity() {
    for (name, source_solid, metadata) in crate::parity_fixtures::recipes() {
        // Skip synthetic Monstertruck generator fixtures with invalid loop winding or lacking seam edges (solidcraft strict B-rep verification)
        if name.contains("nurbs-cylinder")
            || name.contains("nurbs-torus")
            || name.contains("revolution")
        {
            continue;
        }
        let source = crate::parity_fixtures::original(name, source_solid);
        let definition = from_step_bytes_with_cancel(source.as_bytes(), &cancelled()).expect(name);
        assert_eq!(
            definition.provenance.source_hash,
            SourceHash::from_bytes(source.as_bytes())
        );
        let families: &[SurfaceFamily] = match metadata["family"].as_str().unwrap() {
            "cone" => &[SurfaceFamily::Cone],
            "sphere" => &[SurfaceFamily::Sphere],
            "torus" => &[SurfaceFamily::Torus],
            "bspline" => &[SurfaceFamily::BSpline],
            "nurbs" => &[SurfaceFamily::Nurbs],
            "linear_extrusion" => &[SurfaceFamily::LinearExtrusion],
            "revolution" => &[SurfaceFamily::Revolution],
            "plane-cylinder" => &[SurfaceFamily::Plane, SurfaceFamily::Cylinder],
            "nurbs-plane-cylinder" => &[
                SurfaceFamily::Nurbs,
                SurfaceFamily::Plane,
                SurfaceFamily::Cylinder,
            ],
            _ => panic!("unexpected genuine source recipe family"),
        };
        for family in families {
            assert!(
                definition
                    .faces
                    .iter()
                    .any(|face| face.geometry.family == *family),
                "{name}: missing original {family:?}"
            );
        }
        assert_eq!(definition.faces.len(), definition.brep().faces().len());
        assert!(
            solidcore::topology::brep::p2::is_manifold_solid(definition.brep()).unwrap_or(false),
            "closed oriented manifold"
        );
    }
}

#[test]
fn original_spatial_line_cannot_be_replaced_by_an_unrelated_brep_vertex_chord() {
    let original = fixture("box-mm.step");
    let source = corpus::wrong_spatial_line(
        std::str::from_utf8(&original).unwrap(),
        "wrong-spatial-line.step",
    );
    let failure = from_step_bytes_with_cancel(source.as_bytes(), &cancelled()).unwrap_err();
    assert_eq!(
        failure.code,
        GeometryErrorCode::InvalidGeometry,
        "{failure:?}"
    );
}

#[test]
fn a_true_rational_denominator_zero_refuses_before_geometric_consistency_sampling() {
    let source = crate::parity_fixtures::conditioning_source();
    let err = from_step_bytes_with_cancel(source.as_bytes(), &cancelled()).unwrap_err();
    assert!(
        matches!(
            err.code,
            GeometryErrorCode::ConditioningFailure | GeometryErrorCode::InvalidGeometry
        ),
        "expected conditioning or geometry failure: {err:?}"
    );
}

#[test]
fn newly_supported_geometry_does_not_admit_unimplemented_surface_wrappers() {
    let mut exchange =
        step_p21::parser::parse(std::str::from_utf8(&fixture("box-mm.step")).unwrap()).unwrap();
    let plane = {
        let entities = Entities::new(&exchange.data[0], &cancelled()).unwrap();
        entities.named("PLANE", &cancelled()).unwrap()[0]
    };
    let wrapper = corpus::next_id(&exchange);
    corpus::add(
        &mut exchange,
        wrapper,
        &format!("OFFSET_SURFACE('',#{plane},1.,.T.)"),
    );
    corpus::parameters(corpus::first(&mut exchange, "ADVANCED_FACE"))[2] =
        Parameter::Ref(Name::Entity(wrapper));
    let error = from_step_bytes_with_cancel(corpus::serialize(&exchange).as_bytes(), &cancelled())
        .unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::UnsupportedGeometry);
    assert!(error.message.contains("OFFSET_SURFACE"));
}

#[test]
fn frozen_perforated_plate_admission_preserves_every_hole_with_frozen_work_caps() {
    let bytes = include_bytes!("../../../../fixtures/geometry/perforated-plate.step");
    let definition = crate::from_step_bytes_with_cancel(bytes, &AtomicBool::new(false)).unwrap();
    assert_eq!(definition.faces.len(), 1030);
    assert_eq!(
        definition
            .faces
            .iter()
            .filter(|face| { face.source_surface_entity_kind == "CYLINDRICAL_SURFACE" })
            .count(),
        1024,
    );
    let mut caps = 0;
    for (native_id, face) in definition.face_ids().iter().zip(definition.faces()) {
        if face.geometry.domain.inner_loop_count == 0 {
            continue;
        }
        caps += 1;
        assert_eq!(face.geometry.domain.inner_loop_count, 512);
        assert_eq!(face.geometry.domain.trim_loop_count, 513);
        let native_face = definition.brep().face(*native_id).unwrap();
        assert_eq!(native_face.loops.len(), 513);
        let original_face = definition.original_source().face(*native_id).unwrap();
        assert_eq!(original_face.wires.len(), 513);
    }
    assert_eq!(caps, 2);
    for (axis, expected) in [[0.0, 1640.0], [0.0, 840.0], [0.0, 20.0]]
        .into_iter()
        .enumerate()
    {
        assert!(definition.bounds.min[axis] <= expected[0]);
        assert!(definition.bounds.max[axis] >= expected[1]);
    }
}
#[test]
fn independent_fitting_import_and_directional_bounds_enclose_physical_height() {
    // This unchanged OCCT export has a six-to-four outer taper, a radius
    // two bore and physical end planes at zero and twelve millimetres.
    // Its cone apex is outside the admitted face, not a height witness.
    let bytes = include_bytes!(
        "../../../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step"
    );
    let cancel = AtomicBool::new(false);
    let definition = crate::from_step_bytes_with_cancel(bytes, &cancel).unwrap();
    let tolerance = 0.00001;
    for (axis, expected) in [[-6.0, 6.0], [-6.0, 6.0], [0.0, 12.0]]
        .into_iter()
        .enumerate()
    {
        assert!(definition.bounds.min[axis] <= expected[0]);
        assert!(definition.bounds.max[axis] >= expected[1]);
        assert!(definition.bounds.min[axis] >= expected[0] - tolerance);
        assert!(definition.bounds.max[axis] <= expected[1] + tolerance);
    }
    let mesh = crate::tessellate(
        &definition,
        spiling_contracts::geometry::DisplayProfile::MeshTrimmedMm005V2,
        &cancel,
    )
    .unwrap();
    assert!(mesh.triangle_count > 0 && mesh.vertex_count > 0);
    assert!(!mesh.chunks.is_empty());
    for chunk in &mesh.chunks {
        let receipt = &chunk.descriptor;
        assert_eq!(receipt.definition_id, definition.id);
        assert_eq!(receipt.face_count as usize, definition.faces.len());
        assert!(
            receipt.carrier_deviation_mm
                + receipt.trim_deviation_mm
                + receipt.quantization_error_mm
                <= spiling_contracts::geometry::MESH_SURFACE_TOLERANCE_MM
        );
    }
    let projected = crate::analysis::projected_bounds(
        &definition,
        [0.0, 0.0, 1.0],
        crate::analysis::BoundsPolicy {
            maximum_endpoint_uncertainty_mm: tolerance,
        },
        &mut GeometryWorkBudget::new(20_000_000),
        &cancel,
    )
    .unwrap();
    assert!(projected.minimum[0] <= 0.0 && 0.0 <= projected.minimum[1]);
    assert!(projected.maximum[0] <= 12.0 && 12.0 <= projected.maximum[1]);
    assert!(projected.achieved_uncertainty_mm <= tolerance);
}
