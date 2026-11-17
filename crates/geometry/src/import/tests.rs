// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::from_step_bytes_with_cancel;
use crate::{GeometryWorkBudget, corpus_fixtures as corpus};
use std::{collections::BTreeSet, path::PathBuf};
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
    // More entities than the former 100,000 cap are admitted.
    let mut bytes = Vec::new();
    for _ in 0..=150_000 {
        bytes.extend_from_slice(b"#1 /* whitespace */ =A();");
    }
    lexical_bound(&bytes, &cancelled()).unwrap();
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
fn source_object_representation_is_checked_before_file_allocation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("oversized.step");
    std::fs::File::create(&path)
        .unwrap()
        .set_len(u64::from(MAX_SOURCE_BYTES) + 1)
        .unwrap();
    assert_eq!(
        from_step_file(path).unwrap_err().code,
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
        "AP203_CONFIGURATION_CONTROLLED_3D_DESIGN_OF_MECHANICAL_PARTS_AND_ASSEMBLIES_MIM_LF",
        "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF { 1 0 10303 442 1 1 4 }",
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
fn degree_plane_angle_units_scale_angles_without_changing_geometry() {
    let source = String::from_utf8(
        std::fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step"),
        )
        .unwrap(),
    )
    .unwrap();
    let cone = |definition: &crate::Definition| {
        definition
            .faces()
            .iter()
            .find_map(|face| match face.geometry.analytic {
                Some(spiling_contracts::geometry::AnalyticSurfaceDescription::Cone {
                    semi_angle_radians,
                    ..
                }) => Some(semi_angle_radians),
                _ => None,
            })
            .unwrap()
    };
    let radians = from_step_bytes_with_cancel(source.as_bytes(), &cancelled()).unwrap();
    let semi = 0.165148677415_f64;
    let radian_unit = "#199 = ( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );";
    let cone_surface = "#31 = CONICAL_SURFACE('',#32,6.,0.165148677415);";
    assert!(source.contains(radian_unit) && source.contains(cone_surface));
    // Exporters write pi/180 to ten significant digits; both spellings of the
    // derived dimensions are admitted for a plane angle.
    for (factor, named) in [("0.0174532925", "*"), ("1.745329251994E-2", "#9001")] {
        let degrees = source
            .replace(
                radian_unit,
                &format!(
                    "#9000 = ( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );\n\
                     #9001 = DIMENSIONAL_EXPONENTS(0.,0.,0.,0.,0.,0.,0.);\n\
                     #9002 = PLANE_ANGLE_MEASURE_WITH_UNIT(PLANE_ANGLE_MEASURE({factor}),#9000);\n\
                     #199 = ( CONVERSION_BASED_UNIT('DEGREE',#9002) NAMED_UNIT({named}) PLANE_ANGLE_UNIT() );"
                ),
            )
            .replace(
                cone_surface,
                &format!("#31 = CONICAL_SURFACE('',#32,6.,{:.17});", semi.to_degrees()),
            );
        let definition = from_step_bytes_with_cancel(degrees.as_bytes(), &cancelled()).unwrap();
        assert!((cone(&definition) - cone(&radians)).abs() < 1e-14);
        assert_eq!(definition.bounds_mm(), radians.bounds_mm());
        assert_eq!(definition.faces().len(), radians.faces().len());
    }
    // A degree that is not one degree is a different unit, not a rounding.
    let wrong = source.replace(
        radian_unit,
        "#9000 = ( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );\n\
         #9002 = PLANE_ANGLE_MEASURE_WITH_UNIT(PLANE_ANGLE_MEASURE(0.0175),#9000);\n\
         #199 = ( CONVERSION_BASED_UNIT('DEGREE',#9002) NAMED_UNIT(*) PLANE_ANGLE_UNIT() );",
    );
    assert_eq!(
        from_step_bytes_with_cancel(wrong.as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::UnsupportedUnits
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
    let exchange = solidcore_step::p21::parse(std::str::from_utf8(&source).unwrap()).unwrap();
    let expected =
        solidcore_step::read::Reader::new(&exchange, solidcore_step::read::Units::default())
            .surface(822)
            .unwrap();
    assert_eq!(
        original.carrier.as_ref(),
        expected.as_ref(),
        "literal original coefficients and closure declaration"
    );
    let solidcore_step::geom::Surface::BSpline(spline) = expected.as_ref() else {
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
    // The reader clamps the original's inactive end slots; every active basis function, and so
    // the surface on its whole active domain, is the original's (the exact theorem the old
    // decoder used for the same closure).
    for (native, original, multiplicities, degree, count) in [
        (
            native_spline.u_knots(),
            &spline.u_knots,
            &spline.u_multiplicities,
            native_spline.p(),
            native_spline.n() + 1,
        ),
        (
            native_spline.v_knots(),
            &spline.v_knots,
            &spline.v_multiplicities,
            native_spline.q(),
            native_spline.m() + 1,
        ),
    ] {
        let original = expand(original, multiplicities);
        // Only the outermost slots differ, and the active ends have multiplicity `degree`.
        let last = original.len() - 1;
        let clamped = native.len() == original.len()
            && native[1..last] == original[1..last]
            && native[0] == native[degree]
            && native[last] == native[count]
            && original[1..=degree].iter().all(|k| *k == original[degree])
            && original[count..last].iter().all(|k| *k == original[count]);
        assert!(
            native == original.as_slice() || clamped,
            "{native:?} vs {original:?}"
        );
    }
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
            solidcore_step::geom::Curve::SurfaceCurve {
                kind: solidcore_step::geom::SurfaceCurveKind::Seam,
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
    // A LINE moved 123 mm off its edge's vertices is a different edge: refused, never read as the
    // chord between the vertices.
    let original = fixture("box-mm.step");
    let text = std::str::from_utf8(&original).unwrap();
    let source = corpus::wrong_spatial_line(text, "wrong-spatial-line.step");
    let failure = from_step_bytes_with_cancel(source.as_bytes(), &cancelled()).unwrap_err();
    assert_eq!(
        failure.code,
        GeometryErrorCode::InvalidGeometry,
        "{failure:?}"
    );
    assert!(failure.message.contains("off its LINE"), "{failure:?}");
    // Within the import tolerance (the box declares 1e-6 mm; 5e-8 mm off) the LINE
    // is the edge's: accepted, and whatever the reader read another way is reported.
    let near = text.replace(
        "#112 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));",
        "#112 = CARTESIAN_POINT('', (0.0, 5.0E-8, 0.0));",
    );
    assert_ne!(near, text, "the fixture's first LINE point");
    let definition = from_step_bytes_with_cancel(near.as_bytes(), &cancelled()).unwrap();
    for finding in &definition.provenance.findings {
        assert_eq!(
            finding.code,
            spiling_contracts::geometry::ImportFindingCode::SourceReading
        );
        assert!(!finding.detail.is_empty());
    }
}

#[test]
fn a_line_off_its_vertices_within_the_join_tolerance_is_read_and_reported() {
    // ADR012's join tolerance (0.01 mm), not the file's 1e-6 mm, decides: a LINE 0.005 mm off
    // its vertices is the edge's own, read as the segment between them and reported with how
    // far it moved; 0.02 mm off is a different edge and refused.
    let original = fixture("box-mm.step");
    let text = std::str::from_utf8(&original).unwrap();
    let moved = |dy: &str| {
        let source = text.replace(
            "#112 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));",
            &format!("#112 = CARTESIAN_POINT('', (0.0, {dy}, 0.0));"),
        );
        assert_ne!(source, text, "the fixture's first LINE point");
        source
    };
    let definition = from_step_bytes_with_cancel(moved("5.0E-3").as_bytes(), &cancelled()).unwrap();
    let reading = definition
        .provenance
        .findings
        .iter()
        .find(|f| f.detail.contains("off their LINE"))
        .expect("the moved edge is reported");
    assert_eq!(
        reading.code,
        spiling_contracts::geometry::ImportFindingCode::SourceReading
    );
    assert!(reading.count >= 1, "{reading:?}");
    assert_eq!(
        reading.kind,
        spiling_contracts::geometry::ImportFindingKind::Changed
    );
    assert!(
        reading
            .displacement_mm
            .is_some_and(|d| (d - 5.0e-3).abs() < 1e-9),
        "{reading:?}"
    );
    let failure =
        from_step_bytes_with_cancel(moved("2.0E-2").as_bytes(), &cancelled()).unwrap_err();
    assert!(failure.message.contains("off its LINE"), "{failure:?}");
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
    use solidcore_step::geom::Surface as OriginalSurface;

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
            let mut bore_turn = 0.0;
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
                // In the record chart: the native face lies on the source extrusion (every point at
                // its radius from its axis), turns a whole turn about it over its form, spans the
                // eight-mm height, and its trims land on their spatial edges.
                let face = native.face(original.native_face).unwrap();
                let surface = face.surface.as_ref().unwrap();
                let frame = extrusion.position;
                let unit = |v: [f64; 3]| {
                    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    v.map(|c| c / l)
                };
                let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
                let z = unit(frame.axis);
                let r = frame.ref_direction;
                let x = unit([
                    r[0] - dot(r, z) * z[0],
                    r[1] - dot(r, z) * z[1],
                    r[2] - dot(r, z) * z[2],
                ]);
                let y = [
                    z[1] * x[2] - z[2] * x[1],
                    z[2] * x[0] - z[0] * x[2],
                    z[0] * x[1] - z[1] * x[0],
                ];
                let local = |p: [f64; 3]| {
                    let d = [
                        p[0] - frame.origin[0],
                        p[1] - frame.origin[1],
                        p[2] - frame.origin[2],
                    ];
                    [dot(d, x), dot(d, y), dot(d, z)]
                };
                let natural = surface.natural_uv_domain_if_built();
                let (lo, hi) = (natural.min(), natural.max());
                let mut heights = [f64::INFINITY, f64::NEG_INFINITY];
                for a in 0..=8 {
                    for b in 0..=4 {
                        let uv = [
                            lo.x + (hi.x - lo.x) * a as f64 / 8.0,
                            lo.y + (hi.y - lo.y) * b as f64 / 4.0,
                        ];
                        let position = crate::parametric::surface_position_enclosure(
                            surface,
                            [[uv[0]; 2], [uv[1]; 2]],
                            &mut work,
                            &cancel,
                        )
                        .unwrap();
                        let q = local(position.map(|i| i.mid()));
                        assert!(
                            (q[0].hypot(q[1]) - extrusion.radius_mm).abs() <= 1e-9,
                            "{q:?}"
                        );
                        heights = [heights[0].min(q[2]), heights[1].max(q[2])];
                    }
                }
                assert!(
                    heights[0] <= 1e-9 && heights[1] >= 8.0 - 1e-9,
                    "{heights:?}"
                );
                axial_domains.push(heights.map(|h| (h * 1e9).round() / 1e9));
                let chart = crate::parametric::form_chart::FormChart::of(surface)
                    .expect("the bore's cylinder chart");
                let turned = chart.sweep_turn().abs();
                assert!(
                    turned > 0.0 && turned <= std::f64::consts::TAU + 1e-9,
                    "{turned}"
                );
                bore_turn += turned;
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
                    let on = crate::parametric::surface_position_enclosure(
                        surface,
                        [[uv.x; 2], [uv.y; 2]],
                        &mut work,
                        &cancel,
                    )
                    .unwrap()
                    .map(|i| i.mid());
                    let residual =
                        (on[0] - spatial.x).hypot((on[1] - spatial.y).hypot(on[2] - spatial.z));
                    assert!(residual <= native.tolerance());
                    checked_uv += 1;
                }
                assert!(
                    checked_uv > 0,
                    "the angular carrier needs regenerated native trims"
                );
            }
            assert!(circular_faces > 0, "the actual original bore must survive");
            assert!(
                (bore_turn - std::f64::consts::TAU).abs() <= 1e-9,
                "the bore's faces turn {bore_turn}"
            );
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
    use solidcore_step::geom::{Curve, Surface};

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
    let mut checked = 0;
    for original in &definition.original_source().faces {
        let Surface::Extrusion {
            profile, direction, ..
        } = original.carrier.as_ref()
        else {
            continue;
        };
        if !matches!(profile.as_ref(), Curve::Circle { .. }) {
            continue;
        }
        assert_eq!(*direction, [0.0, 0.0, -1.0]);
        let native = definition
            .brep()
            .face(original.native_face)
            .unwrap()
            .surface
            .as_ref()
            .unwrap();
        // Native material admission is independent of this original parameter
        // gauge. Exercise the coefficient association that owns that authority.
        let error =
            match crate::analysis::source_restriction::carrier::SourceCarrierAssociation::new(
                &original.carrier,
                native,
                &mut work,
                &cancel,
            ) {
                Ok(_) => panic!("negative-axis source has no admitted circular-extrusion gauge"),
                Err(error) => error,
            };
        assert_eq!(
            error.code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        );
        checked += 1;
    }
    assert_eq!(
        checked, changed,
        "every changed original bore retains its gauge refusal"
    );
}

fn with_entities(source: &str, entities: &str) -> String {
    let end = source.rfind("ENDSEC;").unwrap();
    format!("{}{entities}\n{}", &source[..end], &source[end..])
}

/// box-mm placed by an assembly usage: rotated 90 degrees about Z and
/// translated to (100, 50, 0), as `instances` usages of one component.
fn box_assembly(instances: usize, parent_first: bool) -> String {
    let source = String::from_utf8(fixture("box-mm.step")).unwrap();
    let mut entities = String::from(
        "#9000=CARTESIAN_POINT('',(0.,0.,0.));\n#9001=DIRECTION('',(0.,0.,1.));\n\
         #9002=DIRECTION('',(1.,0.,0.));\n#9003=AXIS2_PLACEMENT_3D('',#9000,#9001,#9002);\n\
         #9005=DIRECTION('',(0.,0.,1.));\n#9006=DIRECTION('',(0.,1.,0.));\n\
         #9009=PRODUCT_DEFINITION('assembly','',#6,#9);\n\
         #9010=PRODUCT_DEFINITION_SHAPE('','',#9009);\n",
    );
    let mut targets = Vec::new();
    for i in 0..instances {
        let base = 9100 + 10 * i;
        targets.push(format!("#{}", base + 1));
        let (first, second) = if parent_first {
            ("#9008", "#10")
        } else {
            ("#10", "#9008")
        };
        let (one, two) = if parent_first {
            (format!("#{}", base + 1), "#9003".to_string())
        } else {
            ("#9003".to_string(), format!("#{}", base + 1))
        };
        entities += &format!(
            "#{p}=CARTESIAN_POINT('',(100.,{y}.,0.));\n\
             #{a}=AXIS2_PLACEMENT_3D('',#{p},#9005,#9006);\n\
             #{t}=ITEM_DEFINED_TRANSFORMATION('','',{one},{two});\n\
             #{r}=(REPRESENTATION_RELATIONSHIP('','',{first},{second}) \
             REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{t}) SHAPE_REPRESENTATION_RELATIONSHIP());\n\
             #{u}=NEXT_ASSEMBLY_USAGE_OCCURRENCE('{i}','','',#9009,#5,$);\n\
             #{s}=PRODUCT_DEFINITION_SHAPE('','',#{u});\n\
             #{c}=CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{r},#{s});\n",
            p = base,
            a = base + 1,
            y = 50 + 100 * i,
            t = base + 2,
            r = base + 3,
            u = base + 4,
            s = base + 5,
            c = base + 6,
        );
    }
    entities += &format!(
        "#9008=SHAPE_REPRESENTATION('assembly',({}),#11);\n\
         #9011=SHAPE_DEFINITION_REPRESENTATION(#9010,#9008);\n",
        targets.join(",")
    );
    with_entities(&source, &entities)
}

#[test]
fn assembly_usages_place_one_solid_without_changing_its_identity() {
    let plain = from_step_bytes_with_cancel(&fixture("box-mm.step"), &cancelled()).unwrap();
    for parent_first in [false, true] {
        let source = box_assembly(2, parent_first);
        let bodies = structure::source_bodies(source.as_bytes(), &cancelled()).unwrap();
        assert_eq!(bodies.len(), 1);
        let mut placements = bodies[0].placements.clone();
        placements.sort_by(|a, b| a.translation_mm[1].total_cmp(&b.translation_mm[1]));
        assert_eq!(placements.len(), 2);
        let half = std::f64::consts::FRAC_1_SQRT_2;
        for (placement, y) in placements.iter().zip([50.0, 150.0]) {
            for (axis, expected) in [100.0, y, 0.0].into_iter().enumerate() {
                assert!((placement.translation_mm[axis] - expected).abs() < 1e-12);
            }
            for (value, expected) in placement.rotation_xyzw.iter().zip([0.0, 0.0, half, half]) {
                assert!((value - expected).abs() < 1e-12, "{placement:?}");
            }
            // The box's +X edge maps onto +Y: (20, 10, 0) -> (90, y + 20, 0).
            let corner = placement.rotate([20.0, 10.0, 0.0]);
            assert!((corner[0] + placement.translation_mm[0] - 90.0).abs() < 1e-12);
            assert!((corner[1] + placement.translation_mm[1] - (y + 20.0)).abs() < 1e-12);
        }
        // One placed solid keeps the whole-source identity and its own frame.
        let definition = from_step_bytes_with_cancel(source.as_bytes(), &cancelled()).unwrap();
        assert_eq!(definition.bounds_mm(), plain.bounds_mm());
        assert!(definition.provenance().source_solid.is_none());
        let digest: [u8; 32] = Sha256::digest(source.as_bytes()).into();
        assert_eq!(definition.id(), &DefinitionId::from_source_sha256(&digest));
    }
}

#[test]
fn several_solids_require_an_explicit_selector_and_get_distinct_identities() {
    let source = String::from_utf8(fixture("box-mm.step")).unwrap();
    let source = with_entities(
        &source.replace(
            "#10 = SHAPE_REPRESENTATION('', (#16), #11);",
            "#10 = SHAPE_REPRESENTATION('', (#16,#9500), #11);",
        ),
        "#9500=MANIFOLD_SOLID_BREP('second',#17);",
    );
    let bodies = structure::source_bodies(source.as_bytes(), &cancelled()).unwrap();
    let prepared = PreparedStepSource::new(source.as_bytes(), usize::MAX, &cancelled()).unwrap();
    assert_eq!(prepared.bodies(), bodies);
    assert!(prepared.matches_bytes(source.as_bytes()));
    assert!(!prepared.matches_bytes(&fixture("box-mm.step")));
    assert!(prepared.retained_bytes() > size_of::<PreparedStepSource<'_>>());
    assert_eq!(
        bodies
            .iter()
            .map(|b| (b.solid.as_str(), b.name.as_str(), b.placements.len()))
            .collect::<Vec<_>>(),
        [("step:16", "", 1), ("step:9500", "second", 1)]
    );
    assert_eq!(
        from_step_bytes_with_cancel(source.as_bytes(), &cancelled())
            .unwrap_err()
            .code,
        GeometryErrorCode::UnsupportedGeometry
    );
    let digest: [u8; 32] = Sha256::digest(source.as_bytes()).into();
    let mut ids = BTreeSet::new();
    for body in bodies.iter().rev() {
        let definition =
            crate::from_step_solid_with_cancel(source.as_bytes(), &body.solid, &cancelled())
                .unwrap();
        let shared = prepared
            .read_solid(
                Some(&body.solid),
                &cancelled(),
                MAX_NATIVE_FACES,
                MAX_QUERY_PROOF_BYTES,
            )
            .unwrap();
        assert_eq!(shared.id(), definition.id());
        assert_eq!(shared.provenance(), definition.provenance());
        assert_eq!(shared.faces(), definition.faces());
        assert_eq!(shared.bounds_mm(), definition.bounds_mm());
        let plane = spiling_contracts::geometry::PlaneMm {
            origin_mm: [0.0, 0.0, 4.0],
            normal: [0.0, 0.0, 1.0],
        };
        let shared_section = crate::section(&shared, plane, &cancelled()).unwrap();
        let direct_section = crate::section(&definition, plane, &cancelled()).unwrap();
        assert_eq!(shared_section.loops().len(), direct_section.loops().len());
        for (shared_loop, direct_loop) in shared_section.loops().iter().zip(direct_section.loops())
        {
            assert_eq!(shared_loop.is_hole, direct_loop.is_hole);
            assert_eq!(shared_loop.points_mm, direct_loop.points_mm);
        }
        assert_eq!(
            definition.provenance().source_solid.as_ref(),
            Some(&body.solid)
        );
        assert_eq!(
            definition.id(),
            &DefinitionId::from_source_solid(&digest, &body.solid)
        );
        assert_eq!(definition.faces().len(), 6);
        ids.insert(definition.id().clone());
    }
    assert_eq!(ids.len(), 2);
    assert_eq!(
        prepared
            .read_solid(
                Some(&bodies[0].solid),
                &cancelled(),
                5,
                MAX_QUERY_PROOF_BYTES
            )
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit,
    );
    assert_eq!(
        prepared
            .read_solid(
                Some(&bodies[0].solid),
                &AtomicBool::new(true),
                MAX_NATIVE_FACES,
                MAX_QUERY_PROOF_BYTES
            )
            .unwrap_err()
            .code,
        GeometryErrorCode::Cancelled,
    );
    for wrong in ["step:17", "step:999999"] {
        assert_eq!(
            crate::from_step_solid_with_cancel(
                source.as_bytes(),
                &SourceSolidId::parse(wrong).unwrap(),
                &cancelled()
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::InvalidGeometry
        );
    }
}

/// An authored torus cusp. Four quadrant faces of one torus about Z run from a
/// rim circle (v = pi/2, radius `major`, at z = `minor`) down to where the
/// outer sheet meets the axis: a horn torus closes there at its apex and a
/// spindle at its upper cusp, as CAD corner blends do. Without `pit` a planar
/// disk caps the rim and the material is the cusp itself; with `pit` the cusp
/// is a funnel sunk into a cylinder of twice the rim radius whose floor lies
/// 2 mm below it. `degenerate` writes `DEGENERATE_TOROIDAL_SURFACE` with that
/// `select_outer`.
fn torus_cusp_step(major: f64, minor: f64, degenerate: Option<bool>, pit: bool) -> String {
    fn real(x: f64) -> String {
        let s = format!("{x:?}");
        assert!(s.contains('.') && !s.contains('e'), "{s}");
        s
    }
    fn add(data: &mut Vec<String>, text: String) -> String {
        data.push(text);
        format!("#{}", 99 + data.len())
    }
    fn point(d: &mut Vec<String>, p: [f64; 3]) -> String {
        let [x, y, z] = p.map(real);
        add(d, format!("CARTESIAN_POINT('',({x},{y},{z}))"))
    }
    fn vertex(d: &mut Vec<String>, p: [f64; 3]) -> String {
        let p = point(d, p);
        add(d, format!("VERTEX_POINT('',{p})"))
    }
    fn placement(d: &mut Vec<String>, o: [f64; 3], z: [f64; 3], x: [f64; 3]) -> String {
        let o = point(d, o);
        let [z, x] = [z, x].map(|v| {
            let [a, b, c] = v.map(real);
            add(d, format!("DIRECTION('',({a},{b},{c}))"))
        });
        add(d, format!("AXIS2_PLACEMENT_3D('',{o},{z},{x})"))
    }
    fn sense(forward: bool) -> &'static str {
        if forward { ".T." } else { ".F." }
    }
    /// A face of `loops` (the first outer), each a list of oriented edges.
    fn face(
        d: &mut Vec<String>,
        loops: &[&[(&String, bool)]],
        surface: &str,
        same: bool,
    ) -> String {
        let bounds: Vec<String> = loops
            .iter()
            .enumerate()
            .map(|(k, uses)| {
                let uses: Vec<String> = uses
                    .iter()
                    .map(|(e, f)| add(d, format!("ORIENTED_EDGE('',*,*,{e},{})", sense(*f))))
                    .collect();
                let edge_loop = add(d, format!("EDGE_LOOP('',({}))", uses.join(",")));
                let kind = if k == 0 {
                    "FACE_OUTER_BOUND"
                } else {
                    "FACE_BOUND"
                };
                add(d, format!("{kind}('',{edge_loop},.T.)"))
            })
            .collect();
        add(
            d,
            format!(
                "ADVANCED_FACE('',({}),{surface},{})",
                bounds.join(","),
                sense(same)
            ),
        )
    }
    /// Four quarter arcs of `circle` through `at`, counter-clockwise about +Z.
    fn arcs(d: &mut Vec<String>, at: &[String], circle: &str) -> Vec<String> {
        (0..4)
            .map(|i| {
                add(
                    d,
                    format!("EDGE_CURVE('',{},{},{circle},.T.)", at[i], at[(i + 1) % 4]),
                )
            })
            .collect()
    }
    const QUADRANTS: [[f64; 2]; 4] = [[1.0, 0.0], [0.0, 1.0], [-1.0, 0.0], [0.0, -1.0]];
    let up = [0.0, 0.0, 1.0];
    let east = [1.0, 0.0, 0.0];
    let cusp_z = (minor * minor - major * major).sqrt();
    let mut d = Vec::new();
    let origin = placement(&mut d, [0.0; 3], up, east);
    let torus = match degenerate {
        None => format!(
            "TOROIDAL_SURFACE('',{origin},{},{})",
            real(major),
            real(minor)
        ),
        Some(outer) => format!(
            "DEGENERATE_TOROIDAL_SURFACE('',{origin},{},{},{})",
            real(major),
            real(minor),
            sense(outer)
        ),
    };
    let torus = add(&mut d, torus);
    let top = placement(&mut d, [0.0, 0.0, minor], up, east);
    let top_plane = add(&mut d, format!("PLANE('',{top})"));
    let rim_circle = add(&mut d, format!("CIRCLE('',{top},{})", real(major)));
    let cusp = vertex(&mut d, [0.0, 0.0, cusp_z]);
    let rim: Vec<String> = QUADRANTS
        .iter()
        .map(|[c, s]| vertex(&mut d, [major * c, major * s, minor]))
        .collect();
    let rim_arcs = arcs(&mut d, &rim, &rim_circle);
    // Each meridian runs from its rim vertex (angle pi/2) to the cusp.
    let meridians: Vec<String> = QUADRANTS
        .iter()
        .zip(&rim)
        .map(|(&[c, s], rim)| {
            let frame = placement(
                &mut d,
                [major * c, major * s, 0.0],
                [s, -c, 0.0],
                [c, s, 0.0],
            );
            let circle = add(&mut d, format!("CIRCLE('',{frame},{})", real(minor)));
            add(&mut d, format!("EDGE_CURVE('',{rim},{cusp},{circle},.T.)"))
        })
        .collect();
    let mut faces = Vec::new();
    if !pit {
        let rim_loop: Vec<_> = rim_arcs.iter().map(|e| (e, true)).collect();
        faces.push(face(&mut d, &[&rim_loop], &top_plane, true));
        // The material lies outside the tube: against the out-of-tube normal.
        for i in 0..4 {
            let uses = [
                (&rim_arcs[i], false),
                (&meridians[i], true),
                (&meridians[(i + 1) % 4], false),
            ];
            faces.push(face(&mut d, &[&uses], &torus, false));
        }
    } else {
        let (outer, floor) = (2.0 * major, cusp_z - 2.0);
        let wall_circle = add(&mut d, format!("CIRCLE('',{top},{})", real(outer)));
        let bottom = placement(&mut d, [0.0, 0.0, floor], up, east);
        let bottom_plane = add(&mut d, format!("PLANE('',{bottom})"));
        let floor_circle = add(&mut d, format!("CIRCLE('',{bottom},{})", real(outer)));
        let cylinder = add(
            &mut d,
            format!("CYLINDRICAL_SURFACE('',{origin},{})", real(outer)),
        );
        let upper: Vec<String> = QUADRANTS
            .iter()
            .map(|[c, s]| vertex(&mut d, [outer * c, outer * s, minor]))
            .collect();
        let lower: Vec<String> = QUADRANTS
            .iter()
            .map(|[c, s]| vertex(&mut d, [outer * c, outer * s, floor]))
            .collect();
        let upper_arcs = arcs(&mut d, &upper, &wall_circle);
        let lower_arcs = arcs(&mut d, &lower, &floor_circle);
        let down = add(&mut d, "DIRECTION('',(0.0,0.0,-1.0))".into());
        let rulings: Vec<String> = (0..4)
            .map(|i| {
                let vector = add(&mut d, format!("VECTOR('',{down},1.0)"));
                let [c, s] = QUADRANTS[i];
                let start = point(&mut d, [outer * c, outer * s, minor]);
                let line = add(&mut d, format!("LINE('',{start},{vector})"));
                add(
                    &mut d,
                    format!("EDGE_CURVE('',{},{},{line},.T.)", upper[i], lower[i]),
                )
            })
            .collect();
        let outer_loop: Vec<_> = upper_arcs.iter().map(|e| (e, true)).collect();
        let hole: Vec<_> = rim_arcs.iter().rev().map(|e| (e, false)).collect();
        faces.push(face(&mut d, &[&outer_loop, &hole], &top_plane, true));
        // The material lies on the tube's side: along the out-of-tube normal.
        for i in 0..4 {
            let uses = [
                (&rim_arcs[i], true),
                (&meridians[(i + 1) % 4], true),
                (&meridians[i], false),
            ];
            faces.push(face(&mut d, &[&uses], &torus, true));
        }
        for i in 0..4 {
            let j = (i + 1) % 4;
            let uses = [
                (&upper_arcs[i], false),
                (&rulings[i], true),
                (&lower_arcs[i], true),
                (&rulings[j], false),
            ];
            faces.push(face(&mut d, &[&uses], &cylinder, true));
        }
        let floor_loop: Vec<_> = lower_arcs.iter().rev().map(|e| (e, false)).collect();
        faces.push(face(&mut d, &[&floor_loop], &bottom_plane, false));
    }
    let shell = add(&mut d, format!("CLOSED_SHELL('',({}))", faces.join(",")));
    let solid = add(&mut d, format!("MANIFOLD_SOLID_BREP('',{shell})"));
    let mut text = String::from(
        "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('torus cusp'),'2;1');\n\
         FILE_NAME('torus-cusp.step','2026-01-01T00:00:00',('Spiling contributors'),('Spiling'),'','','');\n\
         FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n\
         #1=APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#2);\n\
         #2=APPLICATION_CONTEXT('core data for automotive mechanical design processes');\n\
         #3=SHAPE_DEFINITION_REPRESENTATION(#4,#10);\n#4=PRODUCT_DEFINITION_SHAPE('','',#5);\n\
         #5=PRODUCT_DEFINITION('design','',#6,#9);\n#6=PRODUCT_DEFINITION_FORMATION('','',#7);\n\
         #7=PRODUCT('','','',(#8));\n#8=PRODUCT_CONTEXT('',#2,'mechanical');\n\
         #9=PRODUCT_DEFINITION_CONTEXT('part definition',#2,'design');\n",
    );
    text += &format!("#10=SHAPE_REPRESENTATION('',({solid}),#11);\n");
    text += "#11=(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#15))\
             GLOBAL_UNIT_ASSIGNED_CONTEXT((#12,#13,#14))REPRESENTATION_CONTEXT('',''));\n\
             #12=(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.));\n\
             #13=(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.));\n\
             #14=(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT());\n\
             #15=UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.0E-6),#12,'distance_accuracy_value','');\n";
    for (i, entity) in d.iter().enumerate() {
        text += &format!("#{}={entity};\n", 100 + i);
    }
    text + "ENDSEC;\nEND-ISO-10303-21;\n"
}

#[test]
fn horn_and_spindle_torus_cusps_import_and_section_on_their_outer_sheet() {
    use crate::tolerant::{TolerantModel, TolerantPolicy};
    use spiling_contracts::geometry::{AnalyticSurfaceDescription, TorusKind};
    let cancel = cancelled();
    let policy = TolerantPolicy::default();
    // (major, minor, kind, degenerate record, section height). The cusp's
    // section radius on the outer sheet is major - sqrt(minor^2 - z^2).
    for (major, minor, kind, degenerate, z) in [
        (5.0, 5.0, TorusKind::Horn, None, 3.0),
        (5.0, 10.0, TorusKind::Spindle, None, 9.5),
        (5.0, 10.0, TorusKind::Spindle, Some(true), 9.5),
    ] {
        for pit in [false, true] {
            let label = format!("{major}/{minor} {degenerate:?} pit {pit}");
            let source = torus_cusp_step(major, minor, degenerate, pit);
            let definition = from_step_bytes_with_cancel(source.as_bytes(), &cancel)
                .unwrap_or_else(|e| panic!("{label}: {e:?}"));
            let tori = definition
                .faces()
                .iter()
                .filter(|face| {
                    matches!(
                        face.geometry.analytic,
                        Some(AnalyticSurfaceDescription::Torus { torus_kind, .. }) if torus_kind == kind
                    )
                })
                .count();
            assert_eq!(tori, 4, "{label}");
            let cusp_z = (minor * minor - major * major).sqrt();
            let bounds = definition.bounds_mm();
            // Import boxes are conservative enclosures, not extrema.
            assert!(
                bounds.min[2] <= cusp_z && bounds.max[2] >= minor,
                "{label}: {bounds:?}"
            );
            let model = TolerantModel::build(&definition, policy, &cancel).unwrap();
            assert_eq!(model.failed_faces, 0, "{label}");
            let section = model.section(z).unwrap();
            let radius = major - (minor * minor - z * z).sqrt();
            // The pit's section is the cylinder's circle with the cusp's as a hole.
            let mut rings = vec![(radius, if pit { -1.0 } else { 1.0 })];
            if pit {
                rings.insert(0, (2.0 * major, 1.0));
            }
            let mut loops = section.loops.clone();
            loops.sort_by(|a, b| b.area.total_cmp(&a.area));
            assert_eq!(loops.len(), rings.len(), "{label}");
            for (ring, (r, side)) in loops.iter().zip(rings) {
                let area = side * std::f64::consts::PI * r * r;
                let perimeter = std::f64::consts::TAU * r;
                assert!(
                    (ring.area - area).abs() <= policy.profile_mm * perimeter,
                    "{label}: area {} expected {area}",
                    ring.area
                );
                for p in &ring.points {
                    assert!(
                        (p[0].hypot(p[1]) - r).abs() <= policy.profile_mm,
                        "{label}: {p:?} off radius {r}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_spindle_torus_inner_sheet_is_refused_by_name() {
    let source = torus_cusp_step(5.0, 10.0, Some(false), false);
    let error = from_step_bytes_with_cancel(source.as_bytes(), &cancelled()).unwrap_err();
    assert!(error.message.contains("lemon"), "{error:?}");
}
/// Every reading the reader makes is reported: an edge written against its `same_sense` is read
/// by its vertices, and the definition carries that as a source-reading finding with its count.
#[test]
fn a_reader_reading_of_the_source_is_reported_as_a_finding() {
    let original = fixture("box-mm.step");
    let text = std::str::from_utf8(&original).unwrap();
    let line = text
        .lines()
        .find(|l| l.contains("EDGE_CURVE(") && l.trim_end().ends_with(".T.);"))
        .unwrap();
    let flipped = text.replacen(line, &line.replace(".T.);", ".F.);"), 1);
    assert_ne!(flipped, text);
    let definition = from_step_bytes_with_cancel(flipped.as_bytes(), &cancelled()).unwrap();
    let findings = &definition.provenance.findings;
    assert!(
        findings.iter().any(|f| f.code
            == spiling_contracts::geometry::ImportFindingCode::SourceReading
            && f.count == 1
            && f.detail.contains("same_sense")),
        "{findings:?}"
    );
    assert!(definition.provenance.validate().is_ok());
    assert!(
        from_step_bytes_with_cancel(&original, &cancelled())
            .unwrap()
            .provenance
            .findings
            .is_empty()
    );
}

/// Import bounds are the solid's own extent, not its enclosures' looseness: a whole NURBS
/// cylinder of radius 5 and height 8 (rational quarter arcs, a NURBS floor and roof) bounds
/// to x, y in [-5, 5] and z in [0, 8] within the bounds slack, and contains its own mesh.
#[test]
fn import_bounds_are_tight_and_contain_the_mesh() {
    let definition = crate::analysis::test_source::original_definition(
        crate::analysis::test_source::nurbs_cylinder(),
    );
    let b = &definition.bounds;
    let slack = 2.0 * super::BOUNDS_SLACK_MM + 1e-9;
    for (axis, [lo, hi]) in [[-5.0, 5.0], [-5.0, 5.0], [0.0, 8.0]]
        .into_iter()
        .enumerate()
    {
        assert!(
            b.min[axis] <= lo + 1e-9 && b.min[axis] >= lo - slack,
            "{axis}: {b:?}"
        );
        assert!(
            b.max[axis] >= hi - 1e-9 && b.max[axis] <= hi + slack,
            "{axis}: {b:?}"
        );
    }
    let (plan, _) = solidcore_tess::plan(
        definition.record(),
        &solidcore_tess::MeshTolerance::chord(0.005),
        20.0,
    )
    .unwrap();
    let (mesh, refused) = plan.evaluate();
    assert!(refused.is_empty() && !mesh.positions.is_empty());
    for p in &mesh.positions {
        for (axis, v) in [p.x, p.y, p.z].into_iter().enumerate() {
            assert!(b.min[axis] <= v && v <= b.max[axis], "{p:?} outside {b:?}");
        }
    }
}
