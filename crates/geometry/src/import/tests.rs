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
        let definition = import_step(&source, &cancel).expect("unchanged independent OCCT solid");
        assert_eq!(definition.source_bytes as usize, source.len());
        assert_eq!(definition.provenance.source_hash.as_str(), hash);
        assert_eq!(definition.faces.len(), count);
        for (info, source_face) in definition.faces.iter().zip(canonical) {
            let id = reference(source_face).unwrap();
            assert_eq!(info.face_id, FaceId::from_step_entity(id).unwrap());
            assert_eq!(
                info.surface_face_entity,
                resolve_face(&entities, id).unwrap().to_string()
            );
        }
        let uncertainty = definition
            .provenance
            .uncertainty_mm
            .expect("declared source uncertainty");
        let mut physical_min = [f64::INFINITY; 3];
        let mut physical_max = [f64::NEG_INFINITY; 3];
        let mut hull_budget = [0.0_f64; 3];
        for curve in &definition.boundary_curves {
            for axis in 0..3 {
                let mut include = |value: f64| {
                    physical_min[axis] = physical_min[axis].min(value);
                    physical_max[axis] = physical_max[axis].max(value);
                };
                match curve {
                    BoundaryCurveMm::Line { start_mm, end_mm } => {
                        include(start_mm[axis]);
                        include(end_mm[axis]);
                    }
                    BoundaryCurveMm::CircleArc {
                        center_mm,
                        u_mm,
                        v_mm,
                        start_angle,
                        end_angle,
                    } => {
                        let lo = start_angle.min(*end_angle);
                        let hi = start_angle.max(*end_angle);
                        let radius = u_mm.iter().map(|value| value * value).sum::<f64>().sqrt();
                        assert!(hi - lo <= std::f64::consts::PI + uncertainty / radius);
                        let evaluate = |angle: f64| {
                            center_mm[axis] + u_mm[axis] * angle.cos() + v_mm[axis] * angle.sin()
                        };
                        include(evaluate(lo));
                        include(evaluate(hi));
                        let phase = v_mm[axis].atan2(u_mm[axis]);
                        for stationary in [phase, phase + std::f64::consts::PI] {
                            let angle = lo + (stationary - lo).rem_euclid(std::f64::consts::TAU);
                            if angle <= hi {
                                include(evaluate(angle));
                            }
                        }
                        // Exact rational semicircles, with quarter-parameter knot
                        // insertion, have projected control radius <= sqrt(5)/2
                        // times the physical radius. The certified hull/tube can
                        // therefore overestimate an axis extent by this amount;
                        // it is not a larger physical cylinder.
                        hull_budget[axis] = hull_budget[axis]
                            .max(u_mm[axis].hypot(v_mm[axis]) * (5.0_f64.sqrt() / 2.0 - 1.0));
                    }
                }
            }
        }
        for axis in 0..3 {
            assert!((physical_min[axis] - min[axis]).abs() <= uncertainty);
            assert!((physical_max[axis] - max[axis]).abs() <= uncertainty);
            assert!(definition.bounds.min[axis] <= physical_min[axis]);
            assert!(definition.bounds.max[axis] >= physical_max[axis]);
            let roundoff = 64.0
                * f64::EPSILON
                * physical_min[axis]
                    .abs()
                    .max(physical_max[axis].abs())
                    .max(1.0);
            assert!(
                physical_min[axis] - definition.bounds.min[axis] <= hull_budget[axis] + roundoff
            );
            assert!(
                definition.bounds.max[axis] - physical_max[axis] <= hull_budget[axis] + roundoff
            );
        }
        let shell = &definition.solid.boundaries()[0];
        shell
            .check_solid_boundary()
            .expect("closed oriented manifold");
        assert!(definition.solid.is_geometric_consistent());
        assert!(shell.edge_iter().all(|edge| edge.front() != edge.back()));
        assert!(
            shell
                .iter()
                .any(|face| face.absolute_boundaries().iter().any(|wire| {
                    wire.iter().enumerate().any(|(index, edge)| {
                        wire.iter().skip(index + 1).any(|other| {
                            edge.is_same(other) && edge.orientation() != other.orientation()
                        })
                    })
                })),
            "periodic carrier keeps both shared seam uses in its original face"
        );
        let total_circle_angle: f64 = definition
            .boundary_curves
            .iter()
            .filter_map(|curve| match curve {
                BoundaryCurveMm::CircleArc {
                    start_angle,
                    end_angle,
                    ..
                } => Some((end_angle - start_angle).abs()),
                _ => None,
            })
            .sum();
        assert!((total_circle_angle - 4.0 * std::f64::consts::PI).abs() < 1e-10);
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
        import_step(corpus::serialize(&open).as_bytes(), &cancelled())
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
        import_step(corpus::serialize(&nonperiodic).as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
    let mut inconsistent = original;
    corpus::parameters(corpus::first(&mut inconsistent, "CIRCLE"))[2] = Parameter::Real(4.0);
    assert_eq!(
        import_step(corpus::serialize(&inconsistent).as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::InvalidGeometry
    );
}
