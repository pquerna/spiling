// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::import_step;
use std::path::PathBuf;
use step_p21::ast::{EntityInstance, Name, Parameter};
#[path = "../../examples/corpus/ast.rs"]
mod corpus;
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
        import_step(&bytes, &cancelled()).unwrap_err().code,
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
    let definition = import_step(bytes.as_bytes(), &cancelled()).unwrap();
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
    let same = import_step(bytes.as_bytes(), &cancelled()).unwrap();
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
            let err = import_step(&source, &cancelled())
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
    let mm = import_step(&fixture("box-mm.step"), &cancelled()).unwrap();
    let inch = import_step(&fixture("box-inch.step"), &cancelled()).unwrap();
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
}
#[test]
fn missing_reference_duplicate_ids_and_multiple_data_never_partially_import() {
    let source = String::from_utf8(fixture("box-mm.step")).unwrap();
    let mut ex = step_p21::parser::parse(&source).unwrap();
    let duplicate = ex.data[0].entities[0].clone();
    ex.data[0].entities.push(duplicate);
    let bytes = corpus::serialize(&ex);
    assert_eq!(
        import_step(bytes.as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
    let two_data = source.replace("END-ISO-10303-21;", "DATA; ENDSEC; END-ISO-10303-21;");
    assert_eq!(
        import_step(two_data.as_bytes(), &cancelled())
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
        import_step(bytes.as_bytes(), &cancelled())
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
    let def = import_step(&fixture("cylinder.step"), &cancelled()).unwrap();
    let mut circular = 0;
    for curve in &def.boundary_curves {
        if let BoundaryCurveMm::CircleArc {
            center_mm,
            u_mm,
            v_mm,
            start_angle,
            end_angle,
        } = curve
        {
            circular += 1;
            assert!((Vector3::from(*u_mm).magnitude() - 5.).abs() < 1e-8);
            assert!((Vector3::from(*v_mm).magnitude() - 5.).abs() < 1e-8);
            assert!((center_mm[0]).abs() < 1e-8 && center_mm[1].abs() < 1e-8);
            assert!((end_angle - start_angle).abs() <= std::f64::consts::TAU + 1e-9);
        }
    }
    assert!(circular >= 4, "two analytic half arcs on each cap");
}

#[test]
fn analytic_intersection_leaders_must_lie_on_the_entire_native_carrier() {
    let plane = FaceCarrier::Plane {
        origin_mm: [0.0; 3],
        normal: [0.0, 0.0, 1.0],
    };
    let offset_circle = BoundaryCurveMm::CircleArc {
        center_mm: [0.0, 0.0, 1.0],
        u_mm: [5.0, 0.0, 0.0],
        v_mm: [0.0, 5.0, 0.0],
        start_angle: 0.0,
        end_angle: std::f64::consts::PI,
    };
    assert_eq!(
        certify_boundary(&offset_circle, &plane).unwrap_err().code,
        GeometryErrorCode::InvalidGeometry
    );
    let cylinder = FaceCarrier::Cylinder {
        axis_origin_mm: [0.0; 3],
        axis_direction: [0.0, 0.0, 1.0],
        radius_mm: 5.0,
    };
    // Both endpoints lie on the cylinder, but its interior passes through the axis.
    let chord = BoundaryCurveMm::Line {
        start_mm: [5.0, 0.0, 0.0],
        end_mm: [-5.0, 0.0, 0.0],
    };
    assert_eq!(
        certify_boundary(&chord, &cylinder).unwrap_err().code,
        GeometryErrorCode::InvalidGeometry
    );
    let axial = BoundaryCurveMm::Line {
        start_mm: [5.0, 0.0, 0.0],
        end_mm: [5.0, 0.0, 8.0],
    };
    certify_boundary(&axial, &cylinder).unwrap();
    let circle = BoundaryCurveMm::CircleArc {
        center_mm: [0.0; 3],
        u_mm: [5.0, 0.0, 0.0],
        v_mm: [0.0, 5.0, 0.0],
        start_angle: 0.0,
        end_angle: std::f64::consts::PI,
    };
    certify_boundary(&circle, &plane).unwrap();
    certify_boundary(&circle, &cylinder).unwrap();
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
            import_step(corpus::serialize(&exchange).as_bytes(), &cancelled()).unwrap();
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
        import_step(corpus::serialize(&exchange).as_bytes(), &cancelled())
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
    let definition = import_step(corpus::serialize(&exchange).as_bytes(), &cancel).unwrap();
    assert_eq!(
        definition.provenance.source_unit,
        spiling_contracts::geometry::SourceUnit::Inch
    );
    for axis in 0..3 {
        assert!((definition.bounds.max[axis] - [20.0, 10.0, 8.0][axis]).abs() < 1e-6);
    }
}
