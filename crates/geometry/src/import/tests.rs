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
        for (axis, &coordinate) in point.iter().enumerate() {
            assert!(definition.bounds.min[axis] <= coordinate);
            assert!(definition.bounds.max[axis] >= coordinate);
        }
    }
}

#[test]
fn native_bounds_remain_conservative_at_large_source_origins() {
    let offset = [1_000_000.0, -1_000_000.0, 1_000_000.0];
    for (name, minimum, maximum) in [
        ("box-mm.step", [0.0; 3], [20.0, 10.0, 8.0]),
        ("cylinder.step", [-5.0, -5.0, 0.0], [5.0, 5.0, 8.0]),
    ] {
        let original = fixture(name);
        let mut exchange =
            step_p21::parser::parse(std::str::from_utf8(&original).unwrap()).unwrap();
        for record in corpus::records_mut(&mut exchange) {
            if record.name != "CARTESIAN_POINT" {
                continue;
            }
            let Parameter::List(coordinates) = &mut corpus::parameters(record)[1] else {
                panic!("original point coordinates")
            };
            // UV coordinates stay in their original charts. Translate only
            // spatial points, including the analytic carrier placements.
            if coordinates.len() == 3 {
                for axis in 0..3 {
                    coordinates[axis] =
                        Parameter::Real(number(&coordinates[axis]).unwrap() + offset[axis]);
                }
            }
        }
        let translated = corpus::serialize(&exchange);
        let definition = from_step_bytes_with_cancel(translated.as_bytes(), &cancelled()).unwrap();
        definition.bounds.validate().unwrap();
        for axis in 0..3 {
            assert!(definition.bounds.min[axis] <= minimum[axis] + offset[axis]);
            assert!(definition.bounds.max[axis] >= maximum[axis] + offset[axis]);
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
    for definition in [&mm, &inch] {
        for axis in 0..3 {
            crate::tests::assert_import_axis_extent(
                definition,
                axis,
                [0.0, [20.0, 10.0, 8.0][axis]],
                1e-5,
            );
        }
    }
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
        if let Some(curve) = &edge.curve
            && let solidcore::curve::curve::p2::CurveKind::Ellipse(ell) = curve.kind()
            && ell.is_circle()
        {
            circular += 1;
            assert!((ell.x_radius() - 5.).abs() < 1e-8);
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
            crate::tests::assert_import_axis_extent(
                &definition,
                axis,
                [0.0, [20.0, 10.0, 8.0][axis]],
                1e-6,
            );
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
        crate::tests::assert_import_axis_extent(
            &definition,
            axis,
            [0.0, [20.0, 10.0, 8.0][axis]],
            1e-6,
        );
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
        // Import publishes a conservative scene enclosure, not a precision
        // measurement or a new raw-chart closure certificate. Full-cylinder
        // physical dimensions have their independent source-backed gate.
        for axis in 0..3 {
            assert!(definition.bounds.min[axis] <= min[axis]);
            assert!(definition.bounds.max[axis] >= max[axis]);
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
    let face_id = definition
        .original_source()
        .faces
        .iter()
        .find(|face| face.face_entity.0 == 1301)
        .unwrap()
        .native_face;
    let original = definition.original_source().face(face_id).unwrap();
    assert_eq!(original.carrier_id.0, 822);
    let exchange =
        brep_data_bridge::parse_step_exchange(std::str::from_utf8(&source).unwrap()).unwrap();
    let expected =
        solidcraft_step::read::Reader::new(&exchange, solidcraft_step::read::Units::default())
            .surface(822)
            .unwrap();
    assert_eq!(
        original.carrier.as_ref(),
        expected.as_ref(),
        "literal original coefficients and closure declaration"
    );
    let solidcraft_step::geom::Surface::BSpline(spline) = expected.as_ref() else {
        panic!("original rational spline carrier")
    };
    let native = definition
        .brep()
        .face(face_id)
        .unwrap()
        .surface
        .as_ref()
        .unwrap();
    let native_spline = crate::parametric::analytic_eval::native_surface_nurbs(native)
        .unwrap()
        .unwrap();
    let expand = |knots: &[f64], multiplicities: &[usize]| -> Vec<f64> {
        knots
            .iter()
            .zip(multiplicities)
            .flat_map(|(&k, &m)| std::iter::repeat_n(k, m))
            .collect()
    };
    assert_eq!(
        native_spline.u_knots(),
        expand(&spline.u_knots, &spline.u_multiplicities)
    );
    assert_eq!(
        native_spline.v_knots(),
        expand(&spline.v_knots, &spline.v_multiplicities)
    );
    let association = crate::analysis::source_restriction::carrier::SourceCarrierAssociation::new(
        &original.carrier,
        native,
        &mut GeometryWorkBudget::new(20_000_000),
        &cancelled(),
    )
    .unwrap();
    assert!(
        association.is_literal(),
        "decoder closure must not replace original carrier authority"
    );
    let seam: Vec<_> = original
        .wires
        .iter()
        .flat_map(|w| &w.coedges)
        .map(|&i| definition.original_source().coedge(i).unwrap())
        .filter(|edge| edge.edge_curve_id.0 == 1306)
        .collect();
    assert_eq!(
        seam.len(),
        2,
        "both uses of the original physical seam survive"
    );
    assert_ne!(seam[0].forward, seam[1].forward);
    for use_ in seam {
        assert_eq!(use_.curve_id.0, 1309);
        assert!(matches!(
            use_.spatial_curve.as_ref(),
            solidcraft_step::geom::Curve::SurfaceCurve {
                kind: solidcraft_step::geom::SurfaceCurveKind::Seam,
                ..
            }
        ));
        assert_eq!(
            use_.pcurves
                .iter()
                .map(|p| p.source_pcurve.0)
                .collect::<Vec<_>>(),
            [1313, 1320]
        );
        assert!(use_.pcurves.iter().all(|p| p.carrier_id.0 == 822));
    }
}

#[test]
fn closed_spline_decoder_preserves_active_domain_and_distinct_physical_seam_branches() {
    use solidcraft_geom::brep::{Loop, Surface};
    let source = fixture("independent-exporters/occt-spline-housing-mm.step");
    let mut exchange =
        brep_data_bridge::parse_step_exchange(std::str::from_utf8(&source).unwrap()).unwrap();
    let carrier = exchange.get(822).unwrap().clone();
    let root = *exchange
        .entities
        .iter()
        .find(|(_, e)| e.name() == "MANIFOLD_SOLID_BREP")
        .unwrap()
        .0;
    let mut solid = closed_spline_decode::read_solid(
        &mut exchange,
        root,
        solidcraft_step::units::Context::default(),
        MAX_QUERY_PROOF_BYTES,
        &cancelled(),
    )
    .unwrap();
    assert_eq!(
        exchange.get(822),
        Some(&carrier),
        "source exchange restored before capture"
    );
    let reader =
        solidcraft_step::read::Reader::new(&exchange, solidcraft_step::read::Units::default());
    let source_edge = exchange.get(1306).unwrap().params();
    let seam_vertices = [1, 2].map(|i| {
        let vertex = exchange.get(source_edge[i].as_ref_id().unwrap()).unwrap();
        reader
            .point(vertex.params()[1].as_ref_id().unwrap())
            .unwrap()
    });
    let slot = solid.face_origin.iter().position(|&id| id == 1301).unwrap();
    let face = &solid.brep.faces[slot];
    let Surface::RationalSpline {
        u_knots,
        v_knots,
        homogeneous,
    } = &face.surface
    else {
        panic!("original rational carrier type")
    };
    assert_eq!(
        face.domain,
        [
            [u_knots[2], u_knots[homogeneous.len()]],
            [v_knots[1], v_knots[homogeneous[0].len()]]
        ]
    );
    assert!(
        u_knots[0] < face.domain[0][0] && *u_knots.last().unwrap() > face.domain[0][1],
        "native retains exterior ghost support, not decoder clamping"
    );
    let mut physical_seam = None;
    for (edge, record) in solid.brep.edges.iter().enumerate() {
        let [a, z] = record.uses.as_slice() else {
            continue;
        };
        if a.face as usize != slot || z.face as usize != slot || a.r#loop != z.r#loop {
            continue;
        }
        let Loop::Edges { edges } = &face.loops[a.r#loop as usize] else {
            continue;
        };
        let (a, z) = (&edges[a.position as usize], &edges[z.position as usize]);
        if a.forward != z.forward && a.pcurve != z.pcurve {
            assert_ne!(
                record.vertices[0], record.vertices[1],
                "positive-length seam, not a closed rim"
            );
            let endpoints = record
                .vertices
                .map(|v| solid.brep.vertices[v as usize].point);
            assert!(
                endpoints == seam_vertices || endpoints == [seam_vertices[1], seam_vertices[0]],
                "the native seam retains the original shared physical vertices"
            );
            assert!(
                physical_seam.replace(edge).is_none(),
                "no fabricated additional seam on the original face"
            );
        }
    }
    let seam = physical_seam.expect("real opposite seam uses retain distinct parameter branches");
    brep_data_bridge::strip_lamina_spurs(&mut solid.brep, MAX_QUERY_PROOF_BYTES, &cancelled())
        .unwrap();
    assert_eq!(
        solid.brep.edges[seam].uses.len(),
        2,
        "real seam must not be stripped as a null spur"
    );
    solid.brep.validate().unwrap();
    solid.brep.check_geometry().unwrap();
}

#[test]
fn invalid_closed_spline_declarations_cannot_supply_decoder_periodicity() {
    use solidcraft_step::p21::Param;
    let source = fixture("independent-exporters/occt-spline-housing-mm.step");
    for defect in [
        "not declared closed",
        "unequal endpoint position",
        "unequal endpoint weight",
        "lesser endpoint multiplicity",
    ] {
        let mut exchange =
            brep_data_bridge::parse_step_exchange(std::str::from_utf8(&source).unwrap()).unwrap();
        match defect {
            "not declared closed" => {
                let record = exchange
                    .entities
                    .get_mut(&822)
                    .unwrap()
                    .records
                    .iter_mut()
                    .find(|r| r.name == "B_SPLINE_SURFACE")
                    .unwrap();
                record.params[4] = Param::Enum("F".into());
            }
            "unequal endpoint position" => {
                let record = &mut exchange.entities.get_mut(&835).unwrap().records[0];
                let Param::List(p) = &mut record.params[1] else {
                    panic!("original point")
                };
                p[0] = Param::Real(16.0);
            }
            "unequal endpoint weight" => {
                let record = exchange
                    .entities
                    .get_mut(&822)
                    .unwrap()
                    .records
                    .iter_mut()
                    .find(|r| r.name == "RATIONAL_B_SPLINE_SURFACE")
                    .unwrap();
                let Param::List(rows) = &mut record.params[0] else {
                    panic!("original weights")
                };
                let Param::List(last) = rows.last_mut().unwrap() else {
                    panic!("original weight row")
                };
                last[0] = Param::Real(0.5);
            }
            "lesser endpoint multiplicity" => {
                let record = exchange
                    .entities
                    .get_mut(&822)
                    .unwrap()
                    .records
                    .iter_mut()
                    .find(|r| r.name == "B_SPLINE_SURFACE_WITH_KNOTS")
                    .unwrap();
                let Param::List(m) = &mut record.params[0] else {
                    panic!("original multiplicities")
                };
                // Same vector length/degree/control net, but clamping the
                // ghost now changes active basis functions: outside the theorem.
                m[1] = Param::Int(1);
                m[2] = Param::Int(3);
            }
            _ => unreachable!(),
        }
        let root = *exchange
            .entities
            .iter()
            .find(|(_, e)| e.name() == "MANIFOLD_SOLID_BREP")
            .unwrap()
            .0;
        let original = exchange.get(822).unwrap().clone();
        assert!(
            closed_spline_decode::read_solid(
                &mut exchange,
                root,
                solidcraft_step::units::Context::default(),
                MAX_QUERY_PROOF_BYTES,
                &cancelled(),
            )
            .is_err(),
            "{defect}: no periodic authority from an unqualified declaration"
        );
        assert_eq!(
            exchange.get(822),
            Some(&original),
            "{defect}: original carrier must survive even refusal"
        );
    }
}

#[test]
fn frozen_original_pockets_preserve_source_identity_and_mixed_floor_carriers() {
    for (source, families) in [
        (
            crate::test_sources::BLIND_POCKET,
            &[SurfaceFamily::Plane, SurfaceFamily::Cylinder][..],
        ),
        (
            crate::test_sources::BLIND_POCKET_NURBS_FLOOR,
            &[
                SurfaceFamily::Nurbs,
                SurfaceFamily::Plane,
                SurfaceFamily::Cylinder,
            ][..],
        ),
    ] {
        let definition = from_step_bytes_with_cancel(source, &cancelled()).unwrap();
        assert_eq!(
            definition.provenance.source_hash,
            SourceHash::from_bytes(source)
        );
        assert_eq!(definition.faces.len(), 9);
        assert_eq!(definition.faces.len(), definition.brep().faces().len());
        for (info, &face_id) in definition.faces().iter().zip(definition.face_ids()) {
            let original = definition.original_source().face(face_id).unwrap();
            assert_eq!(original.source_face.0, info.face_id.step_entity());
            let native = definition.brep().face(face_id).unwrap();
            assert_eq!(native.loops.len(), original.wires.len());
            let native_uses: usize = native
                .loops
                .iter()
                .map(|&id| definition.brep().loop_(id).unwrap().edges.len())
                .sum();
            let original_uses: usize = original.wires.iter().map(|wire| wire.coedges.len()).sum();
            assert_eq!(native_uses, original_uses);
        }
        for family in families {
            assert!(
                definition
                    .faces
                    .iter()
                    .any(|face| face.geometry.family == *family)
            );
        }
        assert!(
            solidcore::topology::brep::p2::is_manifold_solid(definition.brep()).unwrap_or(false),
            "closed oriented original pocket"
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
    let err = from_step_bytes_with_cancel(crate::test_sources::DENOMINATOR_ZERO, &cancelled())
        .unwrap_err();
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
    // Physical extents concern the selected source material, not the padded
    // scene box or potentially nonclosed generated native UV trims.
    let mut work = GeometryWorkBudget::new(20_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let query_bytes = crate::MAX_PREPARED_SECTION_BYTES
        .checked_sub(prepared.retained_bytes())
        .unwrap();
    for (axis, expected) in [[-6.0, 6.0], [-6.0, 6.0], [0.0, 12.0]]
        .into_iter()
        .enumerate()
    {
        assert!(definition.bounds.min[axis] <= expected[0]);
        assert!(definition.bounds.max[axis] >= expected[1]);
        let projected = prepared
            .material_bounds(
                std::array::from_fn(|coordinate| if coordinate == axis { 1.0 } else { 0.0 }),
                crate::BoundsPolicy {
                    maximum_endpoint_uncertainty_mm: tolerance,
                },
                query_bytes,
                &mut work,
                &cancel,
            )
            .unwrap()
            .bounds();
        assert!(projected.achieved_uncertainty_mm <= tolerance);
        for (endpoint, value) in [
            (projected.minimum, expected[0]),
            (projected.maximum, expected[1]),
        ] {
            assert!(
                endpoint
                    .into_iter()
                    .all(|bound| (bound - value).abs() <= tolerance)
            );
        }
        if axis == 2 {
            assert!(projected.minimum[0] <= 0.0 && 0.0 <= projected.minimum[1]);
            assert!(projected.maximum[0] <= 12.0 && 12.0 <= projected.maximum[1]);
        }
    }
}

#[test]
fn housing_null_spur_normalization_preserves_original_edges_and_native_ownership() {
    let bytes = include_bytes!(
        "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
    );
    let cancel = cancelled();
    let definition = from_step_bytes_with_cancel(bytes, &cancel).unwrap();
    assert_eq!(
        definition.provenance.source_hash,
        SourceHash::from_digest(&Sha256::digest(bytes).into()),
    );
    let mut work = GeometryWorkBudget::new(20_000_000);
    let mut incidence = crate::analysis::source_incidence::SourceEdgeIncidence::new(
        &definition,
        MAX_QUERY_PROOF_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    incidence
        .prepare_vertices(MAX_QUERY_PROOF_BYTES, &mut work, &cancel)
        .unwrap();
    // This checks reciprocal native loop/vertex/radial ownership, not merely
    // the decoder's copied wire sizes. Every remaining edge is physically paired.
    assert_eq!(incidence.values().count(), definition.brep().edges().len());
    assert!(incidence.values().all(|uses| uses.second.is_some()));
    assert_eq!(
        incidence.vertices().unwrap().count(),
        definition.brep().vertices().len()
    );

    let close = |a: [f64; 3], b: [f64; 3]| (0..3).all(|axis| (a[axis] - b[axis]).abs() <= 1e-10);
    let mut edge_sources = std::collections::HashMap::new();
    let mut source_edges = std::collections::HashMap::new();
    let mut vertex_sources = std::collections::HashMap::new();
    let mut source_vertices = std::collections::HashMap::new();
    let mut observed_pole = false;
    for (info, &face_id) in definition.faces().iter().zip(definition.face_ids()) {
        let original = definition.original_source().face(face_id).unwrap();
        assert_eq!(original.source_face.0, info.face_id.step_entity());
        let native = definition.brep().face(face_id).unwrap();
        assert_eq!(native.loops.len(), original.wires.len());
        let native_uses: usize = native
            .loops
            .iter()
            .map(|&id| definition.brep().loop_(id).unwrap().edges.len())
            .sum();
        let original_uses: usize = original.wires.iter().map(|wire| wire.coedges.len()).sum();
        assert_eq!(
            native_uses,
            original_uses,
            "wire inventory on {}",
            info.face_id.as_str()
        );
        // Endpoint pairs do not identify arbitrary arcs or repeated periodic
        // seam uses. Restrict this physical bijection to the observed spherical
        // octant, whose three original edges have distinct endpoint pairs.
        // Global reciprocal ownership and source inventories remain checked.
        if original.face_entity.0 != 491 {
            continue;
        }
        let source_uses: Vec<_> = original
            .wires
            .iter()
            .enumerate()
            .flat_map(|(wire, source)| source.coedges.iter().map(move |&i| (wire, i)))
            .collect();
        let mut matched = vec![false; source_uses.len()];
        let mut matched_wires = vec![false; original.wires.len()];
        for &loop_id in &native.loops {
            let wire = definition.brep().loop_(loop_id).unwrap();
            assert_eq!(wire.face, face_id);
            let mut source_wire = None;
            for use_ in &wire.edges {
                let edge = definition.brep().edge(use_.edge).unwrap();
                let points = edge
                    .vertices
                    .map(|vertex| array(definition.brep().vertex(vertex).unwrap().point));
                let candidates: Vec<_> = source_uses
                    .iter()
                    .enumerate()
                    .filter(|(slot, (_, coedge))| {
                        if matched[*slot] {
                            return false;
                        }
                        let source = definition.original_source().coedge(*coedge).unwrap();
                        let [a, z] = source.vertex_points_mm;
                        (close(points[0], a) && close(points[1], z))
                            || (close(points[0], z) && close(points[1], a))
                    })
                    .map(|(slot, _)| slot)
                    .collect();
                assert_eq!(
                    candidates.len(),
                    1,
                    "unbound physical edge on {}",
                    info.face_id.as_str()
                );
                let slot = candidates[0];
                matched[slot] = true;
                let (wire_index, coedge) = source_uses[slot];
                assert!(source_wire.is_none_or(|old| old == wire_index));
                source_wire = Some(wire_index);
                let source = definition.original_source().coedge(coedge).unwrap();
                if let Some(old) = edge_sources.insert(use_.edge, source.edge_curve_id.0) {
                    assert_eq!(
                        old, source.edge_curve_id.0,
                        "shared native edge changed source owner"
                    );
                }
                if let Some(old) = source_edges.insert(source.edge_curve_id.0, use_.edge) {
                    assert_eq!(
                        old, use_.edge,
                        "one original edge gained a duplicate native edge"
                    );
                }
                let owners = incidence.get(use_.edge).unwrap();
                for owner in [owners.first, owners.second.unwrap()] {
                    let owner_face = definition.face_ids()[owner.face];
                    let original_owner = definition.original_source().face(owner_face).unwrap();
                    assert!(
                        original_owner
                            .wires
                            .iter()
                            .flat_map(|wire| &wire.coedges)
                            .any(|&i| {
                                definition
                                    .original_source()
                                    .coedge(i)
                                    .unwrap()
                                    .edge_curve_id
                                    == source.edge_curve_id
                            }),
                        "native shared edge must retain its exact original EDGE_CURVE in both source faces",
                    );
                }
                let reversed = !close(points[0], source.vertex_points_mm[0]);
                for endpoint in 0..2 {
                    let source_vertex = source.vertex_ids[endpoint ^ usize::from(reversed)].0;
                    let native_vertex = edge.vertices[endpoint];
                    if let Some(old) = vertex_sources.insert(native_vertex, source_vertex) {
                        assert_eq!(
                            old, source_vertex,
                            "native vertex changed original ownership"
                        );
                    }
                    if let Some(old) = source_vertices.insert(source_vertex, native_vertex) {
                        assert_eq!(
                            old, native_vertex,
                            "original vertex gained a native duplicate"
                        );
                    }
                }
                observed_pole |= points.into_iter().any(|p| close(p, [2.0, 2.0, 10.0]));
                assert!(!points.into_iter().any(|p| close(p, [2.0, 2.0, 6.0])));
            }
            let source_wire = source_wire.unwrap();
            assert!(!std::mem::replace(&mut matched_wires[source_wire], true));
            assert_eq!(wire.edges.len(), original.wires[source_wire].coedges.len());
        }
        assert!(matched.into_iter().all(|matched| matched));
        assert!(matched_wires.into_iter().all(|matched| matched));
    }
    assert!(observed_pole, "the original physical pole must survive");
}

#[test]
fn exact_normalization_retains_the_original_periodic_cylinder_seam_without_scratch() {
    use solidcraft_geom::brep::Loop;
    let text =
        include_str!("../../../../fixtures/geometry/independent-exporters/occt-cylinder-mm.step");
    let exchange = brep_data_bridge::parse_step_exchange(text).unwrap();
    let mut solids = solidcraft_step::topology::read_solids(&exchange);
    assert!(solids.failures.is_empty());
    let mut solid = solids.solids.pop().unwrap();
    let mut seam = None;
    for (edge_id, edge) in solid.brep.edges.iter().enumerate() {
        let [a, z] = edge.uses.as_slice() else {
            continue;
        };
        if a.face != z.face || a.r#loop != z.r#loop {
            continue;
        }
        let Loop::Edges { edges } = &solid.brep.faces[a.face as usize].loops[a.r#loop as usize]
        else {
            panic!("seam must have edge-loop owners")
        };
        let (a, z) = (&edges[a.position as usize], &edges[z.position as usize]);
        if a.forward != z.forward && a.pcurve != z.pcurve {
            seam = Some(edge_id);
        }
    }
    let seam = seam.expect("independent cylinder has a real two-branch periodic seam");
    let before = solid.brep.clone();
    let buffers = (
        solid.brep.faces.as_ptr(),
        solid.brep.edges.as_ptr(),
        solid.brep.vertices.as_ptr(),
    );
    // Zero scratch is enough only because this is not a null-spur candidate.
    brep_data_bridge::strip_lamina_spurs(&mut solid.brep, 0, &cancelled()).unwrap();
    assert_eq!(solid.brep, before);
    assert_eq!(
        buffers,
        (
            solid.brep.faces.as_ptr(),
            solid.brep.edges.as_ptr(),
            solid.brep.vertices.as_ptr(),
        )
    );
    solid.brep.validate().unwrap();
    let cancel = cancelled();
    let ast = step_p21::parser::parse(text).unwrap();
    let entities = Entities::new(&ast.data[0], &cancel).unwrap();
    let mut reader = source::reader(&exchange, 1.0);
    let carriers =
        source::face_carriers(&entities, &mut reader, &solid.face_origin, &cancel).unwrap();
    let mut kernel = crate::kernel::make_kernel_context();
    let native =
        brep_data_bridge::from_schema4_brep(&mut kernel, &solid.brep, &carriers, false).unwrap();
    assert!(solidcore::topology::brep::p2::is_manifold_solid(&native).unwrap());
    let source = &solid.brep.edges[seam];
    assert_ne!(
        source.vertices[0], source.vertices[1],
        "physical seam has positive height"
    );
    let a = solid.brep.vertices[source.vertices[0] as usize].point;
    let z = solid.brep.vertices[source.vertices[1] as usize].point;
    assert!((z[2] - a[2]).abs() > 7.0);
}

#[test]
fn original_implicit_carrier_edges_import_display_bound_and_section_without_branch_reselection() {
    use crate::{AnalysisPolicy, BoundsPolicy, PlaneFrameMm, SlabRequest};
    use spiling_contracts::geometry::RigidPoseMm;
    let frame = PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    for (name, sample, expected_area, minimum, maximum) in [
        ("box-mm.step", 4.0, 200.0, [0.0; 3], [20.0, 10.0, 8.0]),
        ("box-inch.step", 4.0, 200.0, [0.0; 3], [20.0, 10.0, 8.0]),
        (
            "cylinder.step",
            4.0,
            25.0 * std::f64::consts::PI,
            [-5.0, -5.0, 0.0],
            [5.0, 5.0, 8.0],
        ),
        (
            "independent-exporters/occt-tapered-fitting-mm.step",
            6.0,
            21.0 * std::f64::consts::PI,
            [-6.0, -6.0, 0.0],
            [6.0, 6.0, 12.0],
        ),
    ] {
        let original = fixture(name);
        let source = corpus::implicit_intersections(
            std::str::from_utf8(&original).unwrap(),
            "implicit.step",
        );
        let cancel = cancelled();
        let definition = from_step_bytes_with_cancel(source.as_bytes(), &cancel).expect(name);
        let mesh = crate::tessellate(
            &definition,
            crate::DisplayProfile::MeshTrimmedMm005V2,
            &cancel,
        )
        .unwrap_or_else(|error| panic!("original implicit source {name} display: {error}"));
        assert_eq!(mesh.chunks[0].descriptor.definition_id, definition.id);
        for chunk in &mesh.chunks {
            let receipt = &chunk.descriptor;
            assert!(
                receipt.carrier_deviation_mm
                    + receipt.trim_deviation_mm
                    + receipt.quantization_error_mm
                    <= spiling_contracts::geometry::MESH_SURFACE_TOLERANCE_MM
            );
        }
        let mut work = GeometryWorkBudget::new(50_000_000);
        let prepared = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap_or_else(|error| panic!("original implicit source {name} preparation: {error}"));
        let query_bytes = crate::MAX_PREPARED_SECTION_BYTES
            .checked_sub(prepared.retained_bytes())
            .unwrap();
        let native_vertices =
            solidcore::topology::brep::p1::vertices(definition.brep(), None).unwrap();
        for (axis, direction) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .into_iter()
            .enumerate()
        {
            let support = prepared
                .material_bounds(
                    direction,
                    BoundsPolicy {
                        maximum_endpoint_uncertainty_mm: 1e-6,
                    },
                    query_bytes,
                    &mut work,
                    &cancel,
                )
                .unwrap_or_else(|error| {
                    panic!("original implicit source {name} axis {axis}: {error}")
                });
            let bounds = support.bounds();
            // Unit conversion may put an exact native endpoint one ULP from
            // the ideal design dimension. Retain the prior 1e-6 mm reference
            // accuracy for BOTH bracket endpoints, without requiring the
            // design value to lie inside an arbitrarily tight native bracket.
            for (bracket, reference) in [
                (bounds.minimum, minimum[axis]),
                (bounds.maximum, maximum[axis]),
            ] {
                assert!(
                    bracket
                        .into_iter()
                        .all(|end| (end - reference).abs() <= 1e-6),
                    "{name} axis={axis} reference={reference} bounds={bounds:?}"
                );
            }
            // Independently read actual native points: a tighter certificate
            // must still enclose every source-owned native vertex exactly.
            for &vertex in &native_vertices {
                let p = definition.brep().vertex(vertex).unwrap().point;
                let coordinate = [p.x, p.y, p.z][axis];
                assert!(
                    bounds.minimum[0] <= coordinate && coordinate <= bounds.maximum[1],
                    "{name} axis={axis} native_vertex={coordinate} bounds={bounds:?}"
                );
            }
        }
        let interval = [sample - 0.01, sample + 0.01];
        let height = prepared
            .analyze_placed_height_domain(
                RigidPoseMm::IDENTITY,
                interval,
                AnalysisPolicy {
                    maximum_height_uncertainty_mm: 1e-6,
                    maximum_shape_error_mm: 0.1,
                },
                query_bytes,
                &mut work,
                &cancel,
            )
            .unwrap_or_else(|error| {
                panic!("original implicit source {name} height transport: {error}")
            });
        assert!(height.event_brackets_mm().is_empty());
        drop(height);
        let slab = prepared
            .analyze_slab(
                SlabRequest {
                    frame,
                    height_interval_mm: interval,
                    sample_height_mm: sample,
                    native_shape_error_mm: 0.1,
                },
                query_bytes,
                &mut work,
                &cancel,
            )
            .unwrap_or_else(|error| {
                panic!("original implicit source {name} material slab: {error}")
            });
        let section = slab.section();
        assert_eq!(
            section.loops().len(),
            if name.contains("fitting") { 2 } else { 1 }
        );
        assert!(slab.occupied().is_some());
        let mut signed_area = 0.0;
        let mut perimeter = 0.0;
        for boundary in section.loops() {
            for pair in boundary.points_mm.windows(2) {
                signed_area += (pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1]) * 0.5;
                perimeter +=
                    ((pair[1][0] - pair[0][0]).powi(2) + (pair[1][1] - pair[0][1]).powi(2)).sqrt();
            }
        }
        let tolerance = section.sampling_tolerance_mm();
        assert!(
            (signed_area.abs() - expected_area).abs()
                <= 2.0 * perimeter * tolerance
                    + std::f64::consts::TAU * tolerance * tolerance
                    + 1e-8,
            "independent analytic material-area oracle: {name}"
        );
    }
}

#[test]
fn original_circular_extrusion_preserves_angular_law_axial_mm_and_native_uv() {
    use crate::analysis::test_source;
    use crate::kernel::source_operand::original_circular_extrusion;
    use solidcore::topology::edgeuse::p1 as edgeuse;
    use solidcraft_step::geom::Surface as OriginalSurface;

    for rotated in [false, true] {
        let mut bore_axial_domains = None;
        for magnitude in [4.0, 8.0, 16.0] {
            let mut source = test_source::bezier_extrusion(true, rotated);
            for carrier in source.face_surfaces.iter_mut().flatten() {
                if let OriginalSurface::Extrusion {
                    magnitude: scale, ..
                } = carrier
                {
                    // This changes the source-v gauge, not the infinite carrier
                    // or the authored spatial trims of the eight-mm solid.
                    *scale = magnitude;
                }
            }
            let definition = test_source::original_definition(source);
            let native = definition.brep();
            let k = crate::kernel::make_kernel_context();
            let cancel = cancelled();
            let mut work = GeometryWorkBudget::new(1_000_000);
            let mut circular_faces = 0;
            let mut axial_domains = Vec::new();
            for original in &definition.original_source().faces {
                let Some(extrusion) = original_circular_extrusion(&original.carrier) else {
                    continue;
                };
                circular_faces += 1;
                assert_eq!(extrusion.axial_scale_mm, magnitude);
                let ordinal = definition.face_ordinal(original.native_face).unwrap();
                assert_eq!(
                    definition.faces()[ordinal].geometry.family,
                    SurfaceFamily::LinearExtrusion
                );
                let map = definition.face_surface_maps()[ordinal];
                let native_uv = map.step_to_core([0.37, 4.0 / magnitude]);
                assert!((native_uv[0] - 0.37_f64.to_degrees()).abs() < 1e-12);
                assert_eq!(native_uv[1], 4.0);
                let face = native.face(original.native_face).unwrap();
                let lo = face.uv_domain.min();
                let hi = face.uv_domain.max();
                assert_eq!([lo.x, hi.x], [0.0, 360.0]);
                assert!(lo.y <= 0.0 && hi.y >= 8.0);
                axial_domains.push([lo.y, hi.y]);
                let position = crate::parametric::surface_position_enclosure(
                    face.surface.as_ref().unwrap(),
                    [[native_uv[0]; 2], [native_uv[1]; 2]],
                    &mut work,
                    &cancel,
                )
                .unwrap();
                let expected = extrusion.position.at(
                    extrusion.radius_mm * 0.37_f64.cos(),
                    extrusion.radius_mm * 0.37_f64.sin(),
                    4.0,
                );
                for axis in 0..3 {
                    assert!(position[axis].lo <= expected[axis]);
                    assert!(position[axis].hi >= expected[axis]);
                }
                let mut checked_uv = 0;
                for (id, eu) in native.edgeuses().iter() {
                    if edgeuse::face(native, id).unwrap() != Some(original.native_face) {
                        continue;
                    }
                    let uv = definition.edgeuse_uv_trim_curve(id).unwrap();
                    let edge = native.edge(eu.edge.unwrap()).unwrap();
                    let t =
                        edge.interval.min() + 0.37 * (edge.interval.max() - edge.interval.min());
                    let uv = uv.evaluate_point(&k, t).unwrap();
                    let spatial = edge.curve.as_ref().unwrap().evaluate_point(&k, t).unwrap();
                    let angle = uv.x.to_radians();
                    let projected = extrusion.position.at(
                        extrusion.radius_mm * angle.cos(),
                        extrusion.radius_mm * angle.sin(),
                        uv.y,
                    );
                    let residual = (projected[0] - spatial.x)
                        .hypot((projected[1] - spatial.y).hypot(projected[2] - spatial.z));
                    assert!(residual <= native.tolerance());
                    checked_uv += 1;
                }
                assert!(
                    checked_uv > 0,
                    "the angular carrier needs regenerated native trims"
                );
            }
            assert!(circular_faces > 0, "the actual original bore must survive");
            if let Some(expected) = &bore_axial_domains {
                assert_eq!(
                    &axial_domains, expected,
                    "changing source-v magnitude must not rescale decoded cylinder-mm domains"
                );
            } else {
                bore_axial_domains = Some(axial_domains);
            }
        }
    }
}

#[test]
fn negative_axis_circular_extrusion_does_not_acquire_positive_gauge_authority() {
    use crate::analysis::test_source;
    use solidcraft_step::geom::{Curve, Surface};

    let mut source = test_source::bezier_extrusion(true, false);
    let mut changed = 0;
    for carrier in source.face_surfaces.iter_mut().flatten() {
        if let Surface::Extrusion {
            profile, direction, ..
        } = carrier
            && matches!(profile.as_ref(), Curve::Circle { .. })
        {
            *direction = [0.0, 0.0, -1.0];
            changed += 1;
        }
    }
    assert!(changed > 0);
    let definition = test_source::original_definition(source);
    let cancel = cancelled();
    let mut work = GeometryWorkBudget::new(50_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let error = match prepared.source_view() {
        Ok(_) => panic!("negative-axis source has no admitted circular-extrusion gauge"),
        Err(error) => error,
    };
    assert_eq!(
        error.code,
        GeometryErrorCode::ParameterCorrespondenceUnavailable
    );
}
