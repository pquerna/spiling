// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::corpus_fixtures as corpus;
use crate::import_step;
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
    let plane = AnalyticSurfaceDescription::Plane {
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
    let cylinder = AnalyticSurfaceDescription::Cylinder {
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
                    BoundaryCurveMm::Native { .. } => {
                        panic!("analytic independent cylinder must retain circular witnesses")
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

#[test]
fn regular_native_families_import_with_original_face_and_edge_identity() {
    for (name, source_solid, metadata) in crate::parity_fixtures::recipes() {
        let source = crate::parity_fixtures::original(name, source_solid);
        let definition = import_step(source.as_bytes(), &cancelled()).expect(name);
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
        assert_eq!(
            definition.faces.len(),
            definition.solid.boundaries()[0].len()
        );
        assert_eq!(definition.faces.len(), definition.face_boundaries.len());
        let mut source_edges = HashMap::new();
        let mut native_edges = HashMap::new();
        for (face, retained) in definition.solid.boundaries()[0]
            .iter()
            .zip(&definition.face_boundaries)
        {
            assert_eq!(face.id(), retained.face);
            for (wire, refs) in face.absolute_boundaries().iter().zip(&retained.wires) {
                assert_eq!(wire.len(), refs.len());
                for (edge, retained) in wire.iter().zip(refs) {
                    assert_eq!(edge.id(), retained.edge);
                    assert_eq!(edge.orientation(), retained.orientation);
                    bind_native_identity(
                        retained.witness,
                        retained.edge,
                        &mut source_edges,
                        &mut native_edges,
                    )
                    .unwrap();
                    let map = definition.boundary_parameters[retained.witness]
                        .as_ref()
                        .expect("explicit source curve retains its exact parameter map");
                    assert_eq!(map.orientation_preserved(), Some(true));
                    for t in [map.source_range.0, map.source_range.1] {
                        let native_t = map.source_to_native(t).unwrap();
                        assert!((map.native_to_source(native_t).unwrap() - t).abs() < 1e-10);
                    }
                }
            }
        }
    }
}

#[test]
fn retained_parameter_bindings_reject_stale_source_ranges_edge_indices_and_use_sense() {
    let mut definition = import_step(&fixture("box-mm.step"), &cancelled()).unwrap();
    let index = definition.face_boundaries[0].wires[0][0].witness;
    let original = definition.boundary_parameters[index];
    definition.boundary_parameters[index]
        .as_mut()
        .unwrap()
        .source_range
        .1 += 0.5;
    assert_eq!(
        validate_parameter_bindings(&definition, &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::KernelFailure
    );
    definition.boundary_parameters[index] = original;
    definition.face_boundaries[0].wires[0][0].witness = definition.boundary_parameters.len();
    assert_eq!(
        validate_parameter_bindings(&definition, &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::KernelFailure
    );
    definition.face_boundaries[0].wires[0][0].witness = index;
    let coedge = &mut definition.face_boundaries[0].wires[0][0];
    coedge.orientation = !coedge.orientation;
    assert_eq!(
        validate_parameter_bindings(&definition, &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::KernelFailure
    );
}

#[test]
fn original_implicit_carrier_edges_import_display_bound_and_section_without_branch_reselection() {
    use spiling_contracts::geometry::{DisplayProfile, PlaneFrameMm};
    let frame = PlaneFrameMm {
        origin_mm: [0.; 3],
        x_axis: [1., 0., 0.],
        y_axis: [0., 1., 0.],
        z_axis: [0., 0., 1.],
    };
    for (name, sample, area, minimum, maximum) in [
        ("box-mm.step", 4., 200., [0., 0., 0.], [20., 10., 8.]),
        ("box-inch.step", 4., 200., [0., 0., 0.], [20., 10., 8.]),
        (
            "cylinder.step",
            4.,
            25. * std::f64::consts::PI,
            [-5., -5., 0.],
            [5., 5., 8.],
        ),
        (
            "independent-exporters/occt-tapered-fitting-mm.step",
            6.,
            21. * std::f64::consts::PI,
            [-6., -6., 0.],
            [6., 6., 12.],
        ),
    ] {
        let original = fixture(name);
        let source = corpus::implicit_intersections(
            std::str::from_utf8(&original).unwrap(),
            "implicit.step",
        );
        let cancel = cancelled();
        let definition = import_step(source.as_bytes(), &cancel).expect(name);
        crate::tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel).unwrap();
        let mut work = crate::GeometryWorkBudget::new(50_000_000);
        for (axis, direction) in [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
            .into_iter()
            .enumerate()
        {
            let bounds = crate::projected_bounds(
                &definition,
                direction,
                crate::BoundsPolicy {
                    maximum_endpoint_uncertainty_mm: 1e-6,
                },
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(
                bounds.minimum[0] <= minimum[axis] && minimum[axis] <= bounds.minimum[1],
                "{name} axis {axis}"
            );
            assert!(
                bounds.maximum[0] <= maximum[axis] && maximum[axis] <= bounds.maximum[1],
                "{name} axis {axis}"
            );
        }
        let interval = [sample - 0.01, sample + 0.01];
        let height = crate::analyze_height_domain(
            &definition,
            frame,
            interval,
            crate::AnalysisPolicy {
                maximum_height_uncertainty_mm: 1e-6,
                maximum_shape_error_mm: 0.1,
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(height.event_brackets_mm().is_empty());
        drop(height);
        let prepared = crate::prepare_section_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let query_bytes = crate::MAX_PREPARED_SECTION_BYTES
            .checked_sub(prepared.retained_bytes())
            .unwrap();
        let slab = prepared
            .analyze_slab(
                crate::SlabRequest {
                    frame,
                    height_interval_mm: interval,
                    sample_height_mm: sample,
                    native_shape_error_mm: 0.1,
                },
                query_bytes,
                &mut work,
                &cancel,
            )
            .unwrap();
        let section = slab.section();
        assert_eq!(
            section.loops().len(),
            if name.contains("fitting") { 2 } else { 1 }
        );
        assert!(slab.occupied().is_some());
        let mut signed_area = 0.;
        let mut perimeter = 0.;
        for boundary in section.loops() {
            for pair in boundary.points_mm.windows(2) {
                signed_area += (pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1]) * 0.5;
                perimeter +=
                    ((pair[1][0] - pair[0][0]).powi(2) + (pair[1][1] - pair[0][1]).powi(2)).sqrt();
            }
        }
        let tolerance = section.sampling_tolerance_mm();
        assert!(
            (signed_area.abs() - area).abs()
                <= 2. * perimeter * tolerance
                    + std::f64::consts::TAU * tolerance * tolerance
                    + 1e-8,
            "independent analytic material-area oracle: {name}"
        );
    }
}

#[test]
fn original_implicit_query_proof_limit_refuses_and_does_not_poison_later_imports() {
    let original = fixture("box-mm.step");
    let explicit = crate::import_step_with_limits(
        &original,
        &cancelled(),
        crate::ImportLimits {
            max_query_proof_bytes: 0,
            ..crate::ImportLimits::FROZEN
        },
    )
    .unwrap();
    assert_eq!(explicit.retained_query_proof_bytes(), 0);
    let source = corpus::implicit_intersections(
        std::str::from_utf8(&original).unwrap(),
        "proof-budget-implicit-box.step",
    );
    let limits = crate::ImportLimits::FROZEN;
    for max_query_proof_bytes in [0, 1] {
        let failure = crate::import_step_with_limits(
            source.as_bytes(),
            &cancelled(),
            crate::ImportLimits {
                max_query_proof_bytes,
                ..limits
            },
        )
        .unwrap_err();
        assert_eq!(
            failure.code,
            GeometryErrorCode::ResourceLimit,
            "{failure:?}"
        );
    }
    let definition =
        crate::import_step_with_limits(source.as_bytes(), &cancelled(), limits).unwrap();
    let retained = definition.retained_query_proof_bytes();
    assert!(retained > 0);
    let failure = crate::import_step_with_limits(
        source.as_bytes(),
        &cancelled(),
        crate::ImportLimits {
            max_query_proof_bytes: retained - 1,
            ..limits
        },
    )
    .unwrap_err();
    assert_eq!(
        failure.code,
        GeometryErrorCode::ResourceLimit,
        "{failure:?}"
    );
    let recovered =
        crate::import_step_with_limits(source.as_bytes(), &cancelled(), limits).unwrap();
    assert_eq!(definition.id(), recovered.id());
    let pinned_proof_bytes = definition
        .retained_query_proof_bytes()
        .checked_add(recovered.retained_query_proof_bytes())
        .unwrap();
    let frame = PlaneFrameMm {
        origin_mm: [0.; 3],
        x_axis: [1., 0., 0.],
        y_axis: [0., 1., 0.],
        z_axis: [0., 0., 1.],
    };
    for imported in [&definition, &recovered] {
        let mut work = crate::GeometryWorkBudget::new(20_000_000);
        let prepared = crate::prepare_section_source(
            imported,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancelled(),
            &mut work,
        )
        .unwrap();
        let other_proof_bytes = pinned_proof_bytes - imported.retained_query_proof_bytes();
        let query_bytes = crate::MAX_PREPARED_SECTION_BYTES
            .checked_sub(prepared.retained_bytes())
            .and_then(|bytes| bytes.checked_sub(other_proof_bytes))
            .unwrap();
        let request = crate::SlabRequest {
            frame,
            height_interval_mm: [3.99, 4.01],
            sample_height_mm: 4.,
            native_shape_error_mm: 0.1,
        };
        let failure = prepared
            .analyze_slab(request, 0, &mut work, &cancelled())
            .unwrap_err();
        assert_eq!(
            failure.code,
            GeometryErrorCode::ResourceLimit,
            "{failure:?}"
        );
        let slab = prepared
            .analyze_slab(request, query_bytes, &mut work, &cancelled())
            .unwrap();
        let loops = slab.section().loops();
        assert_eq!(loops.len(), 1);
        let area = loops[0]
            .points_mm
            .windows(2)
            .map(|pair| (pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1]) * 0.5)
            .sum::<f64>()
            .abs();
        assert!(
            (area - 200.).abs() < 1e-8,
            "original physical box section must survive failed admissions"
        );
    }
}

#[test]
fn original_implicit_coincident_carriers_do_not_admit_a_leader_as_a_unique_curve() {
    let original = fixture("box-mm.step");
    let source = corpus::implicit_intersections(
        std::str::from_utf8(&original).unwrap(),
        "ambiguous-implicit.step",
    );
    let mut exchange = step_p21::parser::parse(&source).unwrap();
    let declaration = corpus::first(&mut exchange, "INTERSECTION_CURVE");
    let Parameter::List(carriers) = &mut corpus::parameters(declaration)[2] else {
        panic!("controlled carriers");
    };
    carriers[1] = carriers[0].clone();
    let source = corpus::serialize(&exchange);
    let error = import_step(source.as_bytes(), &cancelled()).unwrap_err();
    assert!(
        matches!(
            error.code,
            GeometryErrorCode::AnalysisIndeterminate
                | GeometryErrorCode::SingularGeometry
                | GeometryErrorCode::ConditioningFailure
        ),
        "coincident implicit carriers require a precise regularity refusal: {error:?}"
    );
}

#[test]
fn original_selected_master_on_wrong_world_carrier_cannot_borrow_an_alternate_face_receipt() {
    let source = fixture("independent-exporters/occt-tapered-fitting-mm.step");
    let source = corpus::wrong_selected_master(
        std::str::from_utf8(&source).unwrap(),
        "wrong-selected-master.step",
    );
    let failure = import_step(source.as_bytes(), &cancelled()).unwrap_err();
    assert_eq!(
        failure.code,
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        "{failure:?}"
    );
}

#[test]
fn independent_periodic_uses_bind_their_own_literal_branches_and_native_coverage() {
    use crate::kernel::{TrimClassification, prepare_face_trim_domain_with_byte_limit};
    for name in [
        "independent-exporters/occt-cylinder-mm.step",
        "independent-exporters/occt-tapered-fitting-mm.step",
    ] {
        let source = fixture(name);
        for reverse_associations in [false, true] {
            let mut exchange =
                step_p21::parser::parse(std::str::from_utf8(&source).unwrap()).unwrap();
            if reverse_associations {
                for entity in &mut exchange.data[0].entities {
                    let EntityInstance::Simple { record, .. } = entity else {
                        continue;
                    };
                    if record.name != "SEAM_CURVE" {
                        continue;
                    }
                    let Parameter::List(associated) = &mut corpus::parameters(record)[2] else {
                        panic!("original seam associations");
                    };
                    associated.swap(0, 1);
                }
            }
            let serialized = corpus::serialize(&exchange);
            let cancel = cancelled();
            let definition = import_step(serialized.as_bytes(), &cancel).expect(name);
            assert_eq!(
                definition.faces.len(),
                definition.source_topology.faces.len()
            );
            let mut work = GeometryWorkBudget::new(50_000_000);
            let mut periodic_faces = 0;
            for (index, face) in definition.solid.boundaries()[0].iter().enumerate() {
                let periods = face.with_surface(|surface| [surface.period_u(), surface.period_v()]);
                // Preparation acquires the face's mutex; never do this inside
                // the callback above. Its native arrangement, not these test
                // points, proves the complete original trim's coverage.
                let prepared = prepare_face_trim_domain_with_byte_limit(
                    &definition,
                    index,
                    32 * 1024 * 1024,
                    &mut work,
                    &cancel,
                )
                .expect(name);
                if periods.iter().all(Option::is_none) {
                    continue;
                }
                periodic_faces += 1;
                let domain = prepared.uv_bounds();
                for axis in 0..2 {
                    if let Some(period) = periods[axis] {
                        assert!(
                            (domain[axis][1] - domain[axis][0] - period).abs() < 1e-8,
                            "{name}: original uses must close across one period, not a spurious second sheet"
                        );
                    }
                }
                let center = domain.map(|[a, b]| [(a + b) * 0.5; 2]);
                assert_eq!(
                    prepared.classify(center, &mut work, &cancel).unwrap(),
                    TrimClassification::Inside,
                    "{name}: native chart winding must retain the physical carrier interior"
                );
            }
            assert!(periodic_faces > 0);
        }
    }
}

#[test]
fn periodic_seam_alias_cannot_borrow_a_neighbor_or_preferred_master_branch() {
    let source = fixture("independent-exporters/occt-tapered-fitting-mm.step");
    let mut exchange = step_p21::parser::parse(std::str::from_utf8(&source).unwrap()).unwrap();
    let seam = corpus::first(&mut exchange, "SEAM_CURVE");
    let Parameter::List(associated) = &mut corpus::parameters(seam)[2] else {
        panic!("original seam associations");
    };
    associated[1] = associated[0].clone();
    let serialized = corpus::serialize(&exchange);
    let failure = import_step(serialized.as_bytes(), &cancelled()).unwrap_err();
    assert_eq!(
        failure.code,
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        "{failure:?}"
    );
}

#[test]
fn original_spatial_line_cannot_be_replaced_by_an_unrelated_brep_vertex_chord() {
    let original = fixture("box-mm.step");
    let source = corpus::wrong_spatial_line(
        std::str::from_utf8(&original).unwrap(),
        "wrong-spatial-line.step",
    );
    let failure = import_step(source.as_bytes(), &cancelled()).unwrap_err();
    assert_eq!(
        failure.code,
        GeometryErrorCode::InvalidGeometry,
        "{failure:?}"
    );
}
#[test]
fn source_sphere_chart_is_not_assumed_equal_to_native_uv() {
    let source_solid = crate::parity_fixtures::recipes()
        .into_iter()
        .find(|(name, _, _)| *name == "native-parity-sphere-belt.step")
        .unwrap()
        .1;
    let source = crate::parity_fixtures::original("sphere-chart.step", source_solid);
    let definition = import_step(source.as_bytes(), &cancelled()).unwrap();
    for (info, retained) in definition.faces.iter().zip(&definition.face_boundaries) {
        if info.geometry.family == SurfaceFamily::Sphere {
            let source_uv = (0.25, 0.5);
            let native_uv = retained
                .parameter
                .conversion
                .source_to_native(source_uv)
                .unwrap();
            assert_ne!(
                source_uv, native_uv,
                "sphere longitude/latitude is not native polar/azimuth"
            );
            assert_eq!(definition.provenance.source_unit.scale_to_mm(), 1.0);
            assert!(!info.source_surface_entity_kind.is_empty());
        }
    }
}

/// Failure-only coefficient context stays bounded even if a source has a large
/// tensor net or deeply nested original wrappers.
fn bounded_chart_debug(value: &impl std::fmt::Debug) -> String {
    use std::fmt::Write;
    struct Context(String);
    impl std::fmt::Write for Context {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            let available = 2048_usize.saturating_sub(self.0.len());
            if text.len() <= available {
                self.0.push_str(text);
                return Ok(());
            }
            let mut end = available;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.0.push_str(&text[..end]);
            Err(std::fmt::Error)
        }
    }
    let mut context = Context(String::new());
    if write!(&mut context, "{value:?}").is_err() {
        context.0.push_str("...");
    }
    context.0
}

#[test]
fn original_tensor_frustum_and_pocket_coedges_retain_whole_native_chart_authority() {
    for name in [
        "native-parity-cone-frustum.step",
        "native-parity-blind-pocket.step",
        "native-parity-blind-pocket-nurbs-floor.step",
        "native-parity-nurbs-cylinder.step",
        "native-parity-nurbs-torus.step",
        "native-parity-nurbs-torus-independent-parameters.step",
    ] {
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .unwrap();
        let source = crate::parity_fixtures::original(name, solid);
        let definition = import_step(source.as_bytes(), &cancelled()).expect(name);
        for (face_index, face) in definition.face_boundaries.iter().enumerate() {
            for (wire_index, wire) in face.wires.iter().enumerate() {
                for (coedge_index, coedge) in wire.iter().enumerate() {
                    let native_face = &definition.trimmed_solid.boundaries()[0].faces()[face_index];
                    let native_edge =
                        &native_face.face().absolute_boundaries()[wire_index][coedge_index];
                    assert!(
                        coedge.source_chart_certified,
                        "{name}: face {face_index}, wire {wire_index}, coedge {coedge_index}; \
                         source_kind={} carrier={:?} map={:?} native_range={:?} \
                         chart_range={:?} chart={:?}; original_native_curve={:?}; \
                         carrier_coefficients={:?}",
                        definition.faces[face_index].source_surface_entity_kind,
                        definition.faces[face_index].geometry.family,
                        face.parameter.conversion,
                        native_edge.with_curve(|curve| curve.try_range_tuple()),
                        coedge.source_chart_parameter_range,
                        bounded_chart_debug(&native_face.trims()[wire_index][coedge_index]),
                        native_edge.with_curve(bounded_chart_debug),
                        native_face.face().with_surface(bounded_chart_debug),
                    );
                    let (a, b) = coedge
                        .source_chart_parameter_range
                        .expect("certified original coedge retains its independent chart range");
                    assert!(a.is_finite() && b.is_finite() && a < b);
                }
            }
        }
    }
}

#[test]
fn a_true_rational_denominator_zero_refuses_before_geometric_consistency_sampling() {
    let source = crate::parity_fixtures::conditioning_source();
    assert_eq!(
        import_step(source.as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::ConditioningFailure
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
    let error = import_step(corpus::serialize(&exchange).as_bytes(), &cancelled()).unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::UnsupportedGeometry);
    assert!(error.message.contains("OFFSET_SURFACE"));
}

#[test]
fn sign_equivalent_and_nonzero_mixed_denominators_import_genuine_closed_sources() {
    use monstertruck_io::step::load::step_geometry::re_exports::{
        BsplineSurface, KnotVector, NurbsSurface,
    };
    for mixed in [false, true] {
        let original = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(name, _, _)| *name == "native-parity-nurbs-box.step")
            .unwrap()
            .1;
        let source = original
            .try_mapped(
                |p| Some(*p),
                |c| Some(c.clone()),
                |surface| {
                    let StepSurface::NurbsSurface(surface) = surface else {
                        return None;
                    };
                    if !mixed {
                        let mut negative = surface.clone();
                        for i in 0..2 {
                            for j in 0..2 {
                                *negative.control_point_mut(i, j) *= -1.0;
                            }
                        }
                        return Some(StepSurface::NurbsSurface(negative));
                    }
                    // q has negative interior Bernstein coefficients but q(u)>0:
                    // q = 1 - 1.1*(B_2^5+B_3^5), whose minimum is 0.3125.
                    // Form (q*P(u,v),q) at degree six. Its exact quotient is the
                    // original affine plane, not a fitted or folded replacement.
                    let q = [1.0, 1.0, -0.1, -0.1, 1.0, 1.0];
                    let controls = surface.control_points();
                    let origin = controls[0][0].truncate() / controls[0][0].w;
                    let du = controls[1][0].truncate() / controls[1][0].w - origin;
                    let dv = controls[0][1].truncate() / controls[0][1].w - origin;
                    let net = (0..=6)
                        .map(|i| {
                            let fraction = i as f64 / 6.0;
                            let x = if i == 0 { 0.0 } else { fraction * q[i - 1] };
                            let w = x + if i == 6 { 0.0 } else { (1.0 - fraction) * q[i] };
                            (0..=1)
                                .map(|j| {
                                    let p = w * origin + x * du + (j as f64 * w) * dv;
                                    Vector4::new(p.x, p.y, p.z, w)
                                })
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    Some(StepSurface::NurbsSurface(NurbsSurface::new(
                        BsplineSurface::new(
                            (KnotVector::bezier_knot(6), KnotVector::bezier_knot(1)),
                            net,
                        ),
                    )))
                },
            )
            .unwrap();
        let source = crate::parity_fixtures::original("signed-denominator.step", source);
        let definition = import_step(source.as_bytes(), &cancelled()).unwrap();
        assert_eq!(definition.faces.len(), 6);
        for axis in 0..3 {
            assert!(definition.bounds.min[axis] <= 0.0);
            assert!(definition.bounds.max[axis] >= [20.0, 10.0, 8.0][axis]);
        }
    }
}
