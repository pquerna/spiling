// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod ast;
pub use ast::*;
use step_p21::ast::{EntityInstance, Exchange, Name, Parameter, SubSuperRecord};

/// Original fixed writer IDs: #10 representation, #11 context, #12 length unit.
/// All dimensional geometry and uncertainty are scaled; directions/unit vectors are not.
pub fn inches(mm: &str) -> Exchange {
    let mut exchange = step_p21::parser::parse(mm).unwrap();
    parameters(
        exchange
            .header
            .iter_mut()
            .find(|r| r.name == "FILE_NAME")
            .unwrap(),
    )[0] = Parameter::String("box-inch.step".into());
    for r in records_mut(&mut exchange) {
        match r.name.as_str() {
            "CARTESIAN_POINT" => {
                let Parameter::List(xyz) = &mut parameters(r)[1] else {
                    panic!("point coordinates")
                };
                for p in xyz {
                    let Parameter::Real(v) = p else {
                        panic!("writer uses real coordinates")
                    };
                    *v /= 25.4;
                }
            }
            "VECTOR" | "CIRCLE" | "CYLINDRICAL_SURFACE" => {
                let Parameter::Real(v) = &mut parameters(r)[2] else {
                    panic!("writer length")
                };
                *v /= 25.4;
            }
            "UNCERTAINTY_MEASURE_WITH_UNIT" => {
                let Parameter::Typed { parameter, .. } = &mut parameters(r)[0] else {
                    panic!("writer uncertainty")
                };
                let Parameter::Real(v) = parameter.as_mut() else {
                    panic!("writer uncertainty number")
                };
                *v /= 25.4;
            }
            _ => {}
        }
    }
    let dim = next_id(&exchange);
    let base = dim + 1;
    let factor = dim + 2;
    let replacement: SubSuperRecord =
        format!("(CONVERSION_BASED_UNIT('inch',#{factor}) LENGTH_UNIT() NAMED_UNIT(#{dim}))")
            .parse()
            .unwrap();
    let entity = exchange.data[0]
        .entities
        .iter_mut()
        .find(|e| matches!(e, EntityInstance::Complex { id: 12, .. }))
        .unwrap();
    *entity = EntityInstance::Complex {
        id: 12,
        subsuper: replacement,
    };
    add(
        &mut exchange,
        dim,
        "DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.)",
    );
    let base_unit: SubSuperRecord = "(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.))"
        .parse()
        .unwrap();
    exchange.data[0].entities.push(EntityInstance::Complex {
        id: base,
        subsuper: base_unit,
    });
    add(
        &mut exchange,
        factor,
        &format!("LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE(0.0254),#{base})"),
    );
    exchange
}

pub struct Adversary {
    pub name: &'static str,
    pub mutation: &'static str,
    pub expected: &'static str,
    pub bytes: String,
}
pub fn adversaries(baseline: &str, inch: &str) -> Vec<Adversary> {
    let cases = [
        (
            "missing-units.step",
            "remove_referenced_length_unit",
            "unsupported_units",
        ),
        (
            "conflicting-units.step",
            "add_conflicting_length_unit",
            "unsupported_units",
        ),
        (
            "missing-reference.step",
            "replace_face_carrier_with_missing_reference",
            "invalid_geometry",
        ),
        (
            "open-shell.step",
            "change_closed_shell_to_open_shell",
            "invalid_geometry",
        ),
        (
            "non-manifold-shell.step",
            "remove_one_shell_face",
            "invalid_geometry",
        ),
        (
            "unsupported-nurbs.step",
            "replace_one_plane_with_original_nurbs_carrier",
            "unsupported_geometry",
        ),
        (
            "reachable-unsupported.step",
            "replace_one_boundary_curve_with_polyline",
            "unsupported_geometry",
        ),
        (
            "assembly.step",
            "add_assembly_relationship",
            "unsupported_geometry",
        ),
        (
            "nonfinite-coordinate.step",
            "overflow_one_cartesian_coordinate",
            "invalid_geometry",
        ),
        (
            "truncated.step",
            "truncate_exchange_terminator",
            "invalid_geometry",
        ),
        (
            "wrong-conversion-dimension.step",
            "change_inch_length_exponent_to_two",
            "unsupported_units",
        ),
        (
            "recursive-units.step",
            "make_inch_conversion_reference_itself",
            "unsupported_units",
        ),
        (
            "degrees.step",
            "replace_radian_with_conversion_angle_unit",
            "unsupported_units",
        ),
        (
            "duplicate-face-use.step",
            "duplicate_canonical_shell_face_use",
            "invalid_geometry",
        ),
    ];
    cases.into_iter().map(|(name,mutation,expected)| {
        let mut ex = step_p21::parser::parse(if name == "wrong-conversion-dimension.step" || name == "recursive-units.step" { inch } else { baseline }).unwrap();
        let fresh = next_id(&ex);
        match name {
            "missing-units.step" => { let p = &mut parameters(first(&mut ex,"GLOBAL_UNIT_ASSIGNED_CONTEXT"))[0]; let Parameter::List(v) = p else { panic!() }; v.retain(|p| !matches!(p,Parameter::Ref(Name::Entity(12)))); },
            "conflicting-units.step" => {
                let p = &mut parameters(first(&mut ex,"GLOBAL_UNIT_ASSIGNED_CONTEXT"))[0]; let Parameter::List(v) = p else { panic!() }; v.push(Parameter::Ref(Name::Entity(fresh)));
                let subsuper = "(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT($,.METRE.))".parse().unwrap();
                ex.data[0].entities.push(EntityInstance::Complex { id: fresh, subsuper });
            },
            "missing-reference.step" => parameters(first(&mut ex,"ADVANCED_FACE"))[2] = Parameter::Ref(Name::Entity(fresh)),
            "open-shell.step" => first(&mut ex,"CLOSED_SHELL").name = "OPEN_SHELL".into(),
            "non-manifold-shell.step" => { let Parameter::List(v) = &mut parameters(first(&mut ex,"CLOSED_SHELL"))[1] else { panic!() }; v.pop(); },
            "unsupported-nurbs.step" => {
                parameters(first(&mut ex,"ADVANCED_FACE"))[2] = Parameter::Ref(Name::Entity(fresh));
                for (offset, point) in ["(0.,0.,0.)","(0.,10.,0.)","(20.,0.,0.)","(20.,10.,0.)"].iter().enumerate() {
                    add(&mut ex,fresh+1+offset as u64,&format!("CARTESIAN_POINT('',{point})"));
                }
                let a = fresh+1; let b = fresh+2; let c = fresh+3; let d = fresh+4;
                let subsuper = format!("(BOUNDED_SURFACE() B_SPLINE_SURFACE(1,1,((#{a},#{b}),(#{c},#{d})),.UNSPECIFIED.,.F.,.F.,.F.) B_SPLINE_SURFACE_WITH_KNOTS((2,2),(2,2),(0.,1.),(0.,1.),.UNSPECIFIED.) GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_SURFACE(((1.,1.),(1.,1.))) REPRESENTATION_ITEM('original planar NURBS') SURFACE())").parse().unwrap();
                ex.data[0].entities.push(EntityInstance::Complex { id: fresh, subsuper });
            },
            "reachable-unsupported.step" => {
                let edge = first(&mut ex,"EDGE_CURVE");
                let vertices = [parameters(edge)[1].clone(),parameters(edge)[2].clone()];
                parameters(edge)[3] = Parameter::Ref(Name::Entity(fresh));
                let points = vertices.map(|vertex| {
                    let Parameter::Ref(Name::Entity(vertex_id)) = vertex else { panic!("original edge vertex reference") };
                    let record = ex.data[0].entities.iter().find_map(|e| match e { EntityInstance::Simple { id,record } if *id == vertex_id => Some(record), _ => None }).unwrap();
                    let Parameter::List(ps) = &record.parameter else { panic!() };
                    let Parameter::Ref(Name::Entity(point_id)) = ps[1] else { panic!() };
                    point_id
                });
                add(&mut ex,fresh,&format!("POLYLINE('',(#{},#{}))",points[0],points[1]));
            },
            "assembly.step" => add(&mut ex,fresh,"NEXT_ASSEMBLY_USAGE_OCCURRENCE('original-adversary','','',#5,#5,$)"),
            "nonfinite-coordinate.step" => { let Parameter::List(coordinates) = &mut parameters(first(&mut ex,"CARTESIAN_POINT"))[1] else { panic!() }; coordinates[0] = Parameter::Real(f64::INFINITY); },
            "wrong-conversion-dimension.step" => parameters(first(&mut ex,"DIMENSIONAL_EXPONENTS"))[0] = Parameter::Real(2.0),
            "recursive-units.step" => parameters(first(&mut ex,"LENGTH_MEASURE_WITH_UNIT"))[1] = Parameter::Ref(Name::Entity(12)),
            "degrees.step" => {
                let base = fresh+1;
                let dimension = fresh+2;
                let unit = ex.data[0].entities.iter_mut().find(|e| matches!(e,EntityInstance::Complex { id: 13,.. })).unwrap();
                *unit = EntityInstance::Complex { id: 13, subsuper: format!("(CONVERSION_BASED_UNIT('degree',#{fresh}) NAMED_UNIT(#{dimension}) PLANE_ANGLE_UNIT())").parse().unwrap() };
                let subsuper = "(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.))".parse().unwrap();
                ex.data[0].entities.push(EntityInstance::Complex { id: base, subsuper });
                add(&mut ex,dimension,"DIMENSIONAL_EXPONENTS(0.,0.,0.,0.,0.,0.,0.)");
                add(&mut ex,fresh,&format!("PLANE_ANGLE_MEASURE_WITH_UNIT(PLANE_ANGLE_MEASURE(0.017453292519943295),#{base})"));
            },
            "duplicate-face-use.step" => { let Parameter::List(v) = &mut parameters(first(&mut ex,"CLOSED_SHELL"))[1] else { panic!() }; v.push(v[0].clone()); },
            "truncated.step" => {},
            _ => unreachable!(),
        }
        let mut bytes = serialize(&ex);
        if name == "truncated.step" { bytes.truncate(bytes.len() - 24); }
        Adversary { name,mutation,expected,bytes }
    }).collect()
}
